//! Autenticación en el orden de `cfg.auth`.
//!
//! Presupuesto: `max_auth_attempts` cuenta peticiones al usuario (contraseña,
//! passphrase, rondas de keyboard-interactive). Las identidades de agente y los
//! ficheros de clave no cuentan ahí: tienen un tope propio, como el de un servidor.

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
/// Identidades (agente + ficheros) que se prueban como máximo.
const KEY_TRIES: u32 = 16;
/// Rondas de keyboard-interactive por intento.
const KBD_ROUNDS: u32 = 8;

struct State<'a> {
    handle: &'a mut Handle<Handler>,
    user: &'a str,
    host: &'a str,
    port: u16,
    prompter: &'a dyn AuthPrompter,
    rejected: &'a AtomicBool,
    warnings: &'a mut Vec<String>,
    /// Peticiones al usuario que quedan.
    left: u32,
    exhausted: bool,
    /// Identidades de clave que quedan.
    key_left: u32,
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

    /// Consume una petición al usuario; `false` si ya no quedan.
    fn spend(&mut self) -> bool {
        if self.left == 0 {
            self.exhausted = true;
            return false;
        }
        self.left -= 1;
        true
    }

    /// Consume un intento de identidad; `false` si se alcanzó el tope.
    fn spend_key(&mut self) -> bool {
        if self.key_left == 0 {
            return false;
        }
        self.key_left -= 1;
        true
    }

    /// `true` = autenticado.
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
    warnings: &mut Vec<String>,
) -> Result<()> {
    let mut st = State {
        handle,
        user,
        host: &cfg.host,
        port: cfg.port,
        prompter,
        rejected,
        warnings,
        left: cfg.max_auth_attempts,
        exhausted: false,
        key_left: KEY_TRIES,
        offered: None,
    };
    #[cfg(not(feature = "pkcs11"))]
    if cfg.pkcs11_provider.is_some() {
        st.warnings
            .push("pkcs11_provider ignorado: compila con la feature `pkcs11`".into());
    }
    #[cfg(not(all(feature = "gssapi", unix)))]
    if cfg.gssapi {
        st.warnings
            .push("GSSAPI ignorado: requiere la feature `gssapi` y un sistema Unix".into());
    }
    // GSSAPI va antes que los demás métodos (como OpenSSH); no gasta presupuesto.
    #[cfg(all(feature = "gssapi", unix))]
    if cfg.gssapi && try_gssapi(&mut st, cfg.gssapi_delegate).await? {
        return Ok(());
    }
    for method in &cfg.auth {
        let done = match method {
            AuthMethod::Agent => try_agent(&mut st).await?,
            AuthMethod::PublicKey => {
                #[cfg(feature = "pkcs11")]
                if let Some(p) = &cfg.pkcs11_provider
                    && try_pkcs11(&mut st, p).await?
                {
                    return Ok(());
                }
                try_keys(&mut st, &cfg.identity_files).await?
            }
            AuthMethod::KeyboardInteractive => try_kbd(&mut st).await?,
            AuthMethod::Password => try_password(&mut st).await?,
        };
        if done {
            return Ok(());
        }
    }
    let why = if st.exhausted {
        "se agotaron los intentos permitidos"
    } else {
        "ningún método fue aceptado"
    };
    Err(Error::Auth(format!("{why} ({}@{})", user, cfg.host)))
}

/// La contraseña se repite mientras quede presupuesto y el usuario la dé.
async fn try_password(st: &mut State<'_>) -> Result<bool> {
    while st.offers(MethodKind::Password) && st.spend() {
        let Some(pw) = st.prompter.password_on(st.user, st.host, st.port).await else {
            return Ok(false);
        };
        // `pw` (Zeroizing) se borra al salir; russh recibe una copia suya.
        let r = st
            .handle
            .authenticate_password(st.user, pw.as_str())
            .await
            .map_err(|e| st.err(e))?;
        if st.record(r) {
            return Ok(true);
        }
    }
    Ok(false)
}

async fn try_kbd(st: &mut State<'_>) -> Result<bool> {
    if !st.offers(MethodKind::KeyboardInteractive) {
        return Ok(false);
    }
    let mut resp = st
        .handle
        .authenticate_keyboard_interactive_start(st.user, None)
        .await
        .map_err(|e| st.err(e))?;
    let mut rounds = 0;
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
                rounds += 1;
                if rounds > KBD_ROUNDS {
                    st.warnings.push(format!(
                        "keyboard-interactive: el servidor pidió más de {KBD_ROUNDS} rondas; se abandona"
                    ));
                    return Ok(false);
                }
                let answers: Vec<String> = if prompts.is_empty() {
                    Vec::new()
                } else {
                    if !st.spend() {
                        return Ok(false);
                    }
                    let ps: Vec<(String, bool)> =
                        prompts.into_iter().map(|p| (p.prompt, p.echo)).collect();
                    match st
                        .prompter
                        .keyboard_interactive_for(st.host, &name, &instructions, &ps)
                        .await
                    {
                        Some(a) if a.len() == ps.len() => {
                            a.iter().map(|s| s.as_str().to_owned()).collect()
                        }
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
        if !st.spend_key() {
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

/// `gssapi-with-mic` con el ticket Kerberos del usuario (`kinit`). Sin ticket o sin KDC
/// se avisa y se sigue con el resto de métodos.
#[cfg(all(feature = "gssapi", unix))]
async fn try_gssapi(st: &mut State<'_>, delegate: bool) -> Result<bool> {
    use crate::gssapi::{Gssapi, krb5_oid_der};

    if !st.offers(MethodKind::GssapiWithMic) {
        return Ok(false);
    }
    let mut g = match Gssapi::new(st.host, delegate) {
        Ok(g) => g,
        Err(e) => {
            st.warnings.push(format!("GSSAPI: {e}"));
            return Ok(false);
        }
    };
    let r = st
        .handle
        .authenticate_gssapi_with_mic(st.user, vec![krb5_oid_der()], &mut g)
        .await;
    if let Some(m) = g.server_error.take() {
        st.warnings.push(format!("GSSAPI del servidor: {m}"));
    }
    match r {
        Ok(r) => Ok(st.record(r)),
        Err(e) => {
            st.warnings.push(format!("GSSAPI: {e}"));
            Ok(false)
        }
    }
}

/// Claves de un token PKCS#11 (`pkcs11_provider`). El PIN viene del llavero
/// (`pkcs11-pin:<etiqueta>`) o se pide; nunca de la configuración. Un PIN erróneo
/// no se reintenta: gastaría intentos del token.
#[cfg(feature = "pkcs11")]
async fn try_pkcs11(st: &mut State<'_>, path: &std::path::Path) -> Result<bool> {
    use crate::pkcs11::Module;
    use russh::keys::Algorithm;
    use zeroize::Zeroizing;

    let module = match Module::load(path) {
        Ok(m) => m,
        Err(e) => {
            st.warnings.push(format!("PKCS#11 {}: {e}", path.display()));
            return Ok(false);
        }
    };
    for t in module.tokens() {
        let pin: Option<Zeroizing<String>> = if t.needs_pin {
            match friki_core::secrets::get(&format!("pkcs11-pin:{}", t.label)) {
                Ok(Some(p)) => Some(Zeroizing::new(p)),
                _ => {
                    if !st.spend() {
                        return Ok(false);
                    }
                    let Some(p) = st.prompter.pin(&t.label).await else {
                        continue;
                    };
                    Some(p)
                }
            }
        } else {
            None
        };
        let mut signer = match module.open(&t, pin.as_ref().map(|p| p.as_str())) {
            Ok(s) => s,
            Err(e) => {
                st.warnings.push(format!("PKCS#11 {}: {e}", t.label));
                continue;
            }
        };
        for key in signer.public_keys() {
            if !st.spend_key() {
                return Ok(false);
            }
            let hash = match key.algorithm() {
                Algorithm::Rsa { .. } => st
                    .handle
                    .best_supported_rsa_hash()
                    .await
                    .map_err(|e| st.err(e))?
                    .flatten(),
                _ => None,
            };
            match st
                .handle
                .authenticate_publickey_with(st.user, key, hash, &mut signer)
                .await
            {
                Ok(r) => {
                    if st.record(r) {
                        return Ok(true);
                    }
                }
                Err(e) => st.warnings.push(format!("PKCS#11 {}: {e}", t.label)),
            }
        }
    }
    Ok(false)
}

/// Una clave privada legible por grupo u otros no se usa (como OpenSSH).
#[cfg(unix)]
fn too_open(path: &std::path::Path) -> Option<u32> {
    use std::os::unix::fs::PermissionsExt;
    let mode = std::fs::metadata(path).ok()?.permissions().mode();
    (mode & 0o077 != 0).then_some(mode & 0o777)
}

#[cfg(not(unix))]
fn too_open(_path: &std::path::Path) -> Option<u32> {
    None
}

/// Carga la clave; pide passphrase si está cifrada. `None` = no usable.
async fn load_key(st: &mut State<'_>, path: &std::path::Path) -> Option<russh::keys::PrivateKey> {
    if let Some(mode) = too_open(path) {
        st.warnings.push(format!(
            "clave {} ignorada: permisos {mode:03o} demasiado abiertos (usa chmod 600)",
            path.display()
        ));
        return None;
    }
    match load_secret_key(path, None) {
        Ok(k) => return Some(k),
        Err(russh::keys::Error::KeyIsEncrypted) => {}
        Err(_) => return None,
    }
    for _ in 0..PASSPHRASE_TRIES {
        if !st.spend() {
            return None;
        }
        let pass = st.prompter.passphrase(path).await?;
        if let Ok(k) = load_secret_key(path, Some(pass.as_str())) {
            return Some(k);
        }
    }
    None
}

/// Agente: `SSH_AUTH_SOCK` (socket Unix; en Windows, pipe con nombre), y en Windows
/// además el pipe del agente de OpenSSH y por último Pageant.
async fn try_agent(st: &mut State<'_>) -> Result<bool> {
    if !st.offers(MethodKind::PublicKey) {
        return Ok(false);
    }
    // Sin agente (sin SSH_AUTH_SOCK, socket muerto): se pasa al siguiente método.
    #[cfg(unix)]
    {
        let Ok(agent) = russh::keys::agent::client::AgentClient::connect_env().await else {
            return Ok(false);
        };
        use_agent(st, agent).await
    }
    #[cfg(windows)]
    {
        use russh::keys::agent::client::AgentClient;
        for path in agent_pipes() {
            if let Ok(agent) = AgentClient::connect_named_pipe(path).await {
                return use_agent(st, agent).await;
            }
        }
        let Ok(agent) = AgentClient::connect_pageant().await else {
            return Ok(false);
        };
        use_agent(st, agent).await
    }
}

/// Pipes de agente candidatos en Windows, por orden.
#[cfg(windows)]
pub(crate) fn agent_pipes() -> Vec<std::ffi::OsString> {
    let mut v = Vec::new();
    if let Some(s) = std::env::var_os("SSH_AUTH_SOCK").filter(|s| !s.is_empty()) {
        v.push(s);
    }
    v.push(r"\\.\pipe\openssh-ssh-agent".into());
    v
}

async fn use_agent<S>(
    st: &mut State<'_>,
    mut agent: russh::keys::agent::client::AgentClient<S>,
) -> Result<bool>
where
    S: russh::keys::agent::client::AgentStream + Unpin + Send,
{
    use russh::keys::agent::AgentIdentity;

    let Ok(ids) = agent.request_identities().await else {
        return Ok(false);
    };
    for id in ids {
        let AgentIdentity::PublicKey { key, .. } = id else {
            continue;
        };
        if !st.spend_key() {
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
