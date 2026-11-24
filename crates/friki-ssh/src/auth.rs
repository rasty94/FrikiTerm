//! Autenticación en el orden de `cfg.auth`, con tope global de intentos.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use friki_core::config::{AuthMethod, SessionConfig};
use russh::client::{AuthResult, Handle, KeyboardInteractiveAuthResponse};
use russh::keys::{PrivateKeyWithHashAlg, load_secret_key};
use russh::{MethodKind, MethodSet};

use crate::errors::classify;
use crate::session::Handler;
use crate::{AuthPrompter, Error, Result};

/// Intentos de passphrase por fichero de clave.
const PASSPHRASE_TRIES: usize = 3;

struct State<'a> {
    handle: &'a mut Handle<Handler>,
    user: &'a str,
    host: &'a str,
    prompter: &'a dyn AuthPrompter,
    rejected: &'a AtomicBool,
    left: u32,
    /// Métodos que el servidor aún acepta (se conoce tras el primer fallo).
    offered: Option<MethodSet>,
}

impl State<'_> {
    fn offers(&self, kind: MethodKind) -> bool {
        self.offered.as_ref().is_none_or(|m| m.contains(&kind))
    }

    fn err(&self, e: russh::Error) -> Error {
        classify(e, self.rejected.load(Ordering::SeqCst))
    }

    /// Consume un intento; `false` si ya no quedan.
    fn spend(&mut self) -> bool {
        if self.left == 0 {
            return false;
        }
        self.left -= 1;
        true
    }

    /// `Ok(true)` = autenticado.
    fn record(&mut self, r: AuthResult) -> bool {
        match r {
            AuthResult::Success => true,
            AuthResult::Failure {
                remaining_methods, ..
            } => {
                self.offered = Some(remaining_methods);
                false
            }
        }
    }
}

pub(crate) async fn authenticate(
    handle: &mut Handle<Handler>,
    cfg: &SessionConfig,
    user: &str,
    prompter: &dyn AuthPrompter,
    rejected: &AtomicBool,
) -> Result<()> {
    let mut st = State {
        handle,
        user,
        host: &cfg.host,
        prompter,
        rejected,
        left: cfg.max_auth_attempts,
        offered: None,
    };
    for method in &cfg.auth {
        if st.left == 0 {
            break;
        }
        let done = match method {
            AuthMethod::Agent => try_agent(&mut st).await?,
            AuthMethod::PublicKey => try_keys(&mut st, &cfg.identity_files).await?,
            AuthMethod::KeyboardInteractive => try_kbd(&mut st).await?,
            AuthMethod::Password => try_password(&mut st).await?,
        };
        if done {
            return Ok(());
        }
    }
    let why = if st.left == 0 {
        "se agotaron los intentos permitidos"
    } else {
        "ningún método fue aceptado"
    };
    Err(Error::Auth(format!("{why} ({}@{})", user, cfg.host)))
}

async fn try_password(st: &mut State<'_>) -> Result<bool> {
    if !st.offers(MethodKind::Password) {
        return Ok(false);
    }
    let Some(pw) = st.prompter.password(st.user, st.host).await else {
        return Ok(false);
    };
    if !st.spend() {
        return Ok(false);
    }
    let r = st
        .handle
        .authenticate_password(st.user, pw)
        .await
        .map_err(|e| st.err(e))?;
    Ok(st.record(r))
}

async fn try_kbd(st: &mut State<'_>) -> Result<bool> {
    if !st.offers(MethodKind::KeyboardInteractive) || !st.spend() {
        return Ok(false);
    }
    let mut resp = st
        .handle
        .authenticate_keyboard_interactive_start(st.user, None)
        .await
        .map_err(|e| st.err(e))?;
    loop {
        match resp {
            KeyboardInteractiveAuthResponse::Success => return Ok(true),
            KeyboardInteractiveAuthResponse::Failure {
                remaining_methods, ..
            } => {
                st.offered = Some(remaining_methods);
                return Ok(false);
            }
            KeyboardInteractiveAuthResponse::InfoRequest {
                name,
                instructions,
                prompts,
            } => {
                let answers = if prompts.is_empty() {
                    Vec::new()
                } else {
                    let ps: Vec<(String, bool)> =
                        prompts.into_iter().map(|p| (p.prompt, p.echo)).collect();
                    match st
                        .prompter
                        .keyboard_interactive(&name, &instructions, &ps)
                        .await
                    {
                        Some(a) if a.len() == ps.len() => a,
                        _ => return Ok(false),
                    }
                };
                resp = st
                    .handle
                    .authenticate_keyboard_interactive_respond(answers)
                    .await
                    .map_err(|e| st.err(e))?;
            }
        }
    }
}

fn default_identities() -> Vec<PathBuf> {
    let Some(home) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")) else {
        return Vec::new();
    };
    let ssh = PathBuf::from(home).join(".ssh");
    ["id_ed25519", "id_ecdsa", "id_rsa"]
        .iter()
        .map(|n| ssh.join(n))
        .filter(|p| p.is_file())
        .collect()
}

async fn try_keys(st: &mut State<'_>, files: &[PathBuf]) -> Result<bool> {
    if !st.offers(MethodKind::PublicKey) {
        return Ok(false);
    }
    let files = if files.is_empty() {
        default_identities()
    } else {
        files.to_vec()
    };
    for path in files {
        if st.left == 0 {
            break;
        }
        let Some(key) = load_key(st, &path).await else {
            continue;
        };
        let hash = st
            .handle
            .best_supported_rsa_hash()
            .await
            .map_err(|e| st.err(e))?
            .flatten();
        if !st.spend() {
            break;
        }
        let r = st
            .handle
            .authenticate_publickey(st.user, PrivateKeyWithHashAlg::new(Arc::new(key), hash))
            .await
            .map_err(|e| st.err(e))?;
        if st.record(r) {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Carga la clave; pide passphrase si está cifrada. `None` = no usable.
async fn load_key(st: &State<'_>, path: &std::path::Path) -> Option<russh::keys::PrivateKey> {
    match load_secret_key(path, None) {
        Ok(k) => return Some(k),
        Err(russh::keys::Error::KeyIsEncrypted) => {}
        Err(_) => return None,
    }
    for _ in 0..PASSPHRASE_TRIES {
        let pass = st.prompter.passphrase(path).await?;
        if let Ok(k) = load_secret_key(path, Some(&pass)) {
            return Some(k);
        }
    }
    None
}

#[cfg(unix)]
async fn try_agent(st: &mut State<'_>) -> Result<bool> {
    use russh::keys::agent::{AgentIdentity, client::AgentClient};

    if !st.offers(MethodKind::PublicKey) {
        return Ok(false);
    }
    // Sin agente (sin SSH_AUTH_SOCK, socket muerto): se pasa al siguiente método.
    let Ok(mut agent) = AgentClient::connect_env().await else {
        return Ok(false);
    };
    let Ok(ids) = agent.request_identities().await else {
        return Ok(false);
    };
    for id in ids {
        let AgentIdentity::PublicKey { key, .. } = id else {
            continue;
        };
        if !st.spend() {
            break;
        }
        let hash = st
            .handle
            .best_supported_rsa_hash()
            .await
            .map_err(|e| st.err(e))?
            .flatten();
        // Un agente que se niega a firmar solo descarta esa identidad.
        if let Ok(r) = st
            .handle
            .authenticate_publickey_with(st.user, key, hash, &mut agent)
            .await
            && st.record(r)
        {
            return Ok(true);
        }
    }
    Ok(false)
}

// ponytail: agente solo en Unix (SSH_AUTH_SOCK); Pageant/pipe de Windows cuando se pruebe en Windows.
#[cfg(not(unix))]
async fn try_agent(_st: &mut State<'_>) -> Result<bool> {
    Ok(false)
}
