//! Cliente SSH-2 sobre un crate existente (`russh`). Sin criptografía propia.

mod auth;
mod errors;
mod policy;
mod session;
mod verifier;

use std::path::Path;
use std::sync::Arc;

use friki_core::config::SessionConfig;
use friki_core::known_hosts::HostKey;

pub use errors::Error;
pub use verifier::KnownHostsVerifier;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostKeyDecision {
    Reject,
    AcceptOnce,
    AcceptAndSave,
}

/// Decide sobre la clave del servidor. La implementación por defecto
/// (`KnownHostsVerifier`) consulta `known_hosts` y delega en `ask` lo desconocido.
#[async_trait::async_trait]
pub trait HostKeyVerifier: Send + Sync {
    async fn verify(&self, host: &str, port: u16, key: &HostKey) -> HostKeyDecision;
}

#[async_trait::async_trait]
pub trait AuthPrompter: Send + Sync {
    async fn password(&self, user: &str, host: &str) -> Option<String>;
    async fn passphrase(&self, key_path: &Path) -> Option<String>;
    /// `prompts`: (texto, mostrar eco).
    async fn keyboard_interactive(
        &self,
        name: &str,
        instructions: &str,
        prompts: &[(String, bool)],
    ) -> Option<Vec<String>>;
}

/// Algoritmos ofrecidos, en orden de preferencia (TODO.md §2 y §11).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AlgorithmPolicy {
    pub kex: Vec<&'static str>,
    pub ciphers: Vec<&'static str>,
    pub macs: Vec<&'static str>,
    pub host_keys: Vec<&'static str>,
}

impl AlgorithmPolicy {
    /// Sin 3DES, CBC, RC4, SHA-1, MD5, DH group1 ni DSA. Incluye strict KEX
    /// (las extensiones `ext-info-c` y `kex-strict-c-v00@openssh.com` se añaden
    /// al traducir a `russh::Preferred`; no son algoritmos y no figuran en `kex`).
    pub fn secure() -> Self {
        Self {
            kex: vec![
                "curve25519-sha256",
                "curve25519-sha256@libssh.org",
                "ecdh-sha2-nistp256",
                "ecdh-sha2-nistp384",
                "ecdh-sha2-nistp521",
                "diffie-hellman-group16-sha512",
                "diffie-hellman-group18-sha512",
                "diffie-hellman-group14-sha256",
                "diffie-hellman-group-exchange-sha256",
            ],
            ciphers: vec![
                "chacha20-poly1305@openssh.com",
                "aes256-gcm@openssh.com",
                "aes128-gcm@openssh.com",
                "aes256-ctr",
                "aes192-ctr",
                "aes128-ctr",
            ],
            macs: vec![
                "hmac-sha2-256-etm@openssh.com",
                "hmac-sha2-512-etm@openssh.com",
                "hmac-sha2-256",
                "hmac-sha2-512",
            ],
            host_keys: vec![
                "ssh-ed25519",
                "ecdsa-sha2-nistp256",
                "ecdsa-sha2-nistp384",
                "ecdsa-sha2-nistp521",
                "rsa-sha2-512",
                "rsa-sha2-256",
            ],
        }
    }

    /// `secure()` más los heredados, al final de cada lista. russh los soporta
    /// todos con las características `des` y `dsa` (que activa la feature `legacy`).
    #[cfg(feature = "legacy")]
    pub fn legacy() -> Self {
        let mut p = Self::secure();
        p.kex
            .extend(["diffie-hellman-group14-sha1", "diffie-hellman-group1-sha1"]);
        p.ciphers
            .extend(["aes256-cbc", "aes192-cbc", "aes128-cbc", "3des-cbc"]);
        p.macs.extend(["hmac-sha1-etm@openssh.com", "hmac-sha1"]);
        p.host_keys.extend(["ssh-rsa", "ssh-dss"]);
        p
    }

    /// `legacy()` solo si `cfg.legacy_algorithms` y la feature está activa.
    pub fn for_session(cfg: &SessionConfig) -> Self {
        #[cfg(feature = "legacy")]
        if cfg.legacy_algorithms {
            return Self::legacy();
        }
        let _ = cfg;
        Self::secure()
    }
}

pub struct ExecOutput {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub exit_status: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShellEvent {
    Data(Vec<u8>),
    ExitStatus(u32),
    Closed,
}

pub struct Connection {
    handle: russh::client::Handle<session::Handler>,
}

impl Connection {
    pub async fn connect(
        cfg: &SessionConfig,
        verifier: Arc<dyn HostKeyVerifier>,
        prompter: Arc<dyn AuthPrompter>,
    ) -> Result<Self> {
        let handle = session::connect(cfg, verifier, prompter).await?;
        Ok(Self { handle })
    }

    /// Canal interactivo con PTY.
    pub async fn open_shell(
        &self,
        term: &str,
        cols: u16,
        rows: u16,
    ) -> Result<(ShellWriter, ShellReader)> {
        session::open_shell(&self.handle, term, cols, rows).await
    }

    pub async fn exec(&self, command: &str) -> Result<ExecOutput> {
        session::exec(&self.handle, command).await
    }

    pub async fn disconnect(self) -> Result<()> {
        self.handle
            .disconnect(russh::Disconnect::ByApplication, "", "en")
            .await
            .map_err(|e| errors::classify(e, false))
    }
}

/// Extremo de escritura; clonable para usarlo desde varias tareas.
#[derive(Clone)]
pub struct ShellWriter {
    tx: tokio::sync::mpsc::Sender<session::ShellCmd>,
}

impl ShellWriter {
    pub async fn write(&self, data: &[u8]) -> Result<()> {
        self.request(|ack| session::ShellCmd::Data(data.to_vec(), ack))
            .await
    }

    pub async fn resize(&self, cols: u16, rows: u16) -> Result<()> {
        self.request(|ack| session::ShellCmd::Resize(cols, rows, ack))
            .await
    }

    async fn request(
        &self,
        make: impl FnOnce(tokio::sync::oneshot::Sender<Result<()>>) -> session::ShellCmd,
    ) -> Result<()> {
        let (ack, done) = tokio::sync::oneshot::channel();
        let closed = || Error::Network("canal cerrado".into());
        self.tx.send(make(ack)).await.map_err(|_| closed())?;
        done.await.map_err(|_| closed())?
    }
}

pub struct ShellReader {
    rx: tokio::sync::mpsc::Receiver<ShellEvent>,
}

impl ShellReader {
    /// `None` cuando el canal ya está cerrado.
    pub async fn next_event(&mut self) -> Option<ShellEvent> {
        self.rx.recv().await
    }
}

pub mod keys {
    use std::io::Write;
    use std::path::{Path, PathBuf};

    use ssh_key::{Algorithm, LineEnding, PrivateKey};

    use super::{Error, Result};

    /// Genera una clave Ed25519 en formato OpenSSH (privada en `path`, 0600;
    /// pública en `path.pub`) y devuelve la línea pública. Falla si `path` ya existe.
    pub fn generate_ed25519(
        path: &Path,
        comment: &str,
        passphrase: Option<&str>,
    ) -> Result<String> {
        let other = |e: &dyn std::fmt::Display| Error::Other(format!("generar clave: {e}"));
        let mut rng = rand::rng();
        let mut key = PrivateKey::random(&mut rng, Algorithm::Ed25519).map_err(|e| other(&e))?;
        key.set_comment(comment);
        let public = key.public_key().to_openssh().map_err(|e| other(&e))?;

        let stored = match passphrase.filter(|p| !p.is_empty()) {
            Some(p) => key.encrypt(&mut rng, p).map_err(|e| other(&e))?,
            None => key,
        };
        let pem = stored.to_openssh(LineEnding::LF).map_err(|e| other(&e))?;

        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut opts, 0o600);
        let mut f = opts.open(path).map_err(|e| other(&e))?;
        f.write_all(pem.as_bytes()).map_err(|e| other(&e))?;

        let mut pub_path = path.as_os_str().to_owned();
        pub_path.push(".pub");
        std::fs::write(PathBuf::from(pub_path), format!("{public}\n")).map_err(|e| other(&e))?;
        Ok(public)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn genera_ed25519_con_y_sin_passphrase() {
        let dir = tempfile::tempdir().expect("tempdir");
        let plain = dir.path().join("id");
        let line = crate::keys::generate_ed25519(&plain, "c@h", None).expect("keygen");
        assert!(line.starts_with("ssh-ed25519 ") && line.ends_with(" c@h"));
        assert_eq!(
            std::fs::read_to_string(dir.path().join("id.pub"))
                .expect("pub")
                .trim(),
            line
        );
        assert!(russh::keys::load_secret_key(&plain, None).is_ok());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&plain)
                .expect("meta")
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        // No pisa una clave existente.
        assert!(crate::keys::generate_ed25519(&plain, "c", None).is_err());

        let enc = dir.path().join("enc");
        crate::keys::generate_ed25519(&enc, "c", Some("secreto")).expect("keygen cifrada");
        assert!(russh::keys::load_secret_key(&enc, None).is_err());
        assert!(russh::keys::load_secret_key(&enc, Some("secreto")).is_ok());
    }
}
