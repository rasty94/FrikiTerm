//! Lógica de sesión compartida por la TUI y la interfaz de escritorio: preguntas al usuario
//! (`Asker`), verificación de clave de host, conexión SSH y apertura de flujos por protocolo.

pub mod pubkey;
mod target;

use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;

use friki_core::config::{Forward, Protocol, SessionConfig};
use friki_core::known_hosts::{HostKey, HostKeyStatus};
use friki_core::transport::{StreamReader, StreamWriter};
use friki_ssh::{AuthPrompter, Connection, Error, HostKeyVerifier, KnownHostsVerifier, Negotiated};
use serde::Serialize;

pub use friki_ssh::{ForwardHandle, HostKeyDecision, Zeroizing};
pub use target::{forward_label, label, tag, telnet_options, term_for};

/// Un prompt de la conexión.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum Prompt {
    HostKeyUnknown {
        host: String,
        port: u16,
        algorithm: String,
        fingerprint: String,
    },
    HostKeyChanged {
        host: String,
        port: u16,
        algorithm: String,
        fingerprint: String,
        previous_fingerprint: String,
    },
    Password {
        user: String,
        host: String,
        /// Puerto del servidor que pide la contraseña (puede ser un salto de ProxyJump).
        port: u16,
    },
    Passphrase {
        key_path: String,
    },
    KbdInteractive {
        host: String,
        name: String,
        instructions: String,
        prompts: Vec<KbdPrompt>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct KbdPrompt {
    pub text: String,
    /// Mostrar lo que se escribe.
    pub echo: bool,
}

/// Respuesta a un `Prompt`; `None` en los secretos = cancelado.
pub enum Reply {
    HostKey(HostKeyDecision),
    Secret(Option<Zeroizing<String>>),
    Multi(Option<Vec<Zeroizing<String>>>),
}

/// Quien responde a los prompts (diálogo de la TUI, modal de la GUI...). `None` = sin respuesta.
#[async_trait::async_trait]
pub trait Asker: Send + Sync + Clone + 'static {
    async fn ask(&self, p: Prompt) -> Option<Reply>;
}

/// `AuthPrompter` de friki-ssh sobre un `Asker`.
pub struct Prompter<A: Asker> {
    asker: A,
    /// Destino, para keyboard-interactive sin salto.
    host: String,
}

impl<A: Asker> Prompter<A> {
    pub fn new(asker: A, host: impl Into<String>) -> Self {
        Self {
            asker,
            host: host.into(),
        }
    }

    async fn secret(&self, p: Prompt) -> Option<Zeroizing<String>> {
        match self.asker.ask(p).await? {
            Reply::Secret(s) => s,
            _ => None,
        }
    }
}

#[async_trait::async_trait]
impl<A: Asker> AuthPrompter for Prompter<A> {
    async fn password(&self, user: &str, host: &str) -> Option<Zeroizing<String>> {
        // ponytail: friki-ssh siempre llama a `password_on`; sin puerto solo llega 0.
        self.password_on(user, host, 0).await
    }

    async fn password_on(&self, user: &str, host: &str, port: u16) -> Option<Zeroizing<String>> {
        self.secret(Prompt::Password {
            user: user.into(),
            host: host.into(),
            port,
        })
        .await
    }

    async fn passphrase(&self, key_path: &Path) -> Option<Zeroizing<String>> {
        self.secret(Prompt::Passphrase {
            key_path: key_path.display().to_string(),
        })
        .await
    }

    async fn keyboard_interactive(
        &self,
        name: &str,
        instructions: &str,
        prompts: &[(String, bool)],
    ) -> Option<Vec<Zeroizing<String>>> {
        self.keyboard_interactive_for(&self.host, name, instructions, prompts)
            .await
    }

    /// El servidor que de verdad pregunta (un salto de ProxyJump o el destino).
    async fn keyboard_interactive_for(
        &self,
        host: &str,
        name: &str,
        instructions: &str,
        prompts: &[(String, bool)],
    ) -> Option<Vec<Zeroizing<String>>> {
        let p = Prompt::KbdInteractive {
            host: host.into(),
            name: name.into(),
            instructions: instructions.into(),
            prompts: prompts
                .iter()
                .map(|(text, echo)| KbdPrompt {
                    text: text.clone(),
                    echo: *echo,
                })
                .collect(),
        };
        match self.asker.ask(p).await? {
            Reply::Multi(m) => m,
            _ => None,
        }
    }
}

type Ask = Pin<Box<dyn std::future::Future<Output = HostKeyDecision> + Send>>;

/// Verificador de clave de host sobre `known_hosts`; lo desconocido o cambiado se pregunta.
pub fn verifier<A: Asker>(asker: A, known_hosts: PathBuf) -> Arc<dyn HostKeyVerifier> {
    Arc::new(KnownHostsVerifier::new(
        known_hosts,
        move |host: &str, port: u16, key: &HostKey, status: HostKeyStatus| -> Ask {
            let fingerprint = key.fingerprint_sha256();
            let (host, algorithm) = (host.to_string(), key.algorithm.clone());
            let prompt = match status {
                HostKeyStatus::Unknown => Prompt::HostKeyUnknown {
                    host,
                    port,
                    algorithm,
                    fingerprint,
                },
                HostKeyStatus::Changed { previous } => Prompt::HostKeyChanged {
                    host,
                    port,
                    algorithm,
                    fingerprint,
                    previous_fingerprint: previous.fingerprint_sha256(),
                },
                _ => return Box::pin(async { HostKeyDecision::Reject }),
            };
            let asker = asker.clone();
            Box::pin(async move {
                match asker.ask(prompt).await {
                    Some(Reply::HostKey(d)) => d,
                    _ => HostKeyDecision::Reject,
                }
            })
        },
    ))
}

/// Error de red genérico para los transportes que no son SSH.
pub fn net(e: &std::io::Error) -> anyhow::Error {
    Error::Network(e.to_string()).into()
}

/// Conecta por SSH (clave de host y autenticación vía `asker`).
pub async fn connect_ssh<A: Asker>(
    asker: A,
    session: &SessionConfig,
    known_hosts: PathBuf,
) -> friki_ssh::Result<Connection> {
    let v = verifier(asker.clone(), known_hosts);
    let p = Arc::new(Prompter::new(asker, session.host.clone()));
    Connection::connect(session, v, p).await
}

/// Flujo abierto: la conexión SSH (si la hay) y sus metadatos.
pub struct Opened {
    /// Solo SSH. Compartible: el shell y p. ej. SFTP usan `&Connection`.
    pub conn: Option<Arc<Connection>>,
    pub writer: Arc<dyn StreamWriter>,
    /// (columnas, filas) con las que se abrió el PTY.
    pub size: (u16, u16),
    pub warnings: Vec<String>,
    pub negotiated: Option<Negotiated>,
    /// Resultado de cada redirección de la sesión; un fallo no tumba el shell.
    pub forwards: Vec<(Forward, Result<ForwardHandle, String>)>,
}

/// SSH: conexión, shell con PTY y las redirecciones de la sesión.
pub async fn open_ssh<A: Asker>(
    asker: A,
    session: &SessionConfig,
    known_hosts: PathBuf,
    (cols, rows): (u16, u16),
    term: &str,
) -> anyhow::Result<(Opened, Box<dyn StreamReader>)> {
    let conn = connect_ssh(asker, session, known_hosts).await?;
    let (writer, reader) = conn.open_shell_stream(term, cols, rows).await?;
    let mut forwards = vec![];
    for f in &session.forwards {
        let r = conn.start_forward(f).await.map_err(|e| e.to_string());
        forwards.push((f.clone(), r));
    }
    let opened = Opened {
        warnings: conn.warnings(),
        negotiated: conn.negotiated(),
        conn: Some(Arc::new(conn)),
        writer,
        size: (cols, rows),
        forwards,
    };
    Ok((opened, reader))
}

/// Abre el flujo según el protocolo de la sesión.
pub async fn open_stream<A: Asker>(
    asker: A,
    session: &SessionConfig,
    known_hosts: PathBuf,
    size: (u16, u16),
) -> anyhow::Result<(Opened, Box<dyn StreamReader>)> {
    let term = term_for(session);
    let (cols, rows) = size;
    let stream = match session.protocol {
        Protocol::Ssh => return open_ssh(asker, session, known_hosts, size, &term).await,
        Protocol::Tn3270 if !cfg!(feature = "tn3270") => {
            return Err(anyhow::anyhow!("TN3270 no incluido en esta compilación"));
        }
        Protocol::Telnet | Protocol::TelnetTls | Protocol::Raw | Protocol::Tn3270 => {
            let opts = telnet_options(session, &term);
            friki_telnet::connect(&session.host, session.port, &opts, cols, rows).await
        }
        Protocol::Serial => friki_serial::open(&session.serial).await,
        Protocol::LocalShell => {
            friki_pty::spawn(session.local_command.as_deref(), &term, cols, rows).await
        }
    }
    .map_err(|e| net(&e))?;
    let (writer, reader) = stream;
    let opened = Opened {
        conn: None,
        writer,
        size,
        warnings: vec![],
        negotiated: None,
        forwards: vec![],
    };
    Ok((opened, reader))
}

/// Cierra la conexión si es la última referencia; si no, se cierra al soltar la última.
pub async fn disconnect(conn: Arc<Connection>) {
    if let Ok(c) = Arc::try_unwrap(conn) {
        let _ = c.disconnect().await;
    }
}

#[cfg(test)]
mod tests;
