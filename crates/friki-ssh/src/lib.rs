//! Cliente SSH-2 sobre un crate existente (`russh`). Sin criptografía propia.

use std::path::Path;
use std::sync::Arc;

use friki_core::config::SessionConfig;
use friki_core::known_hosts::HostKey;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Resolución, conexión, timeout o corte de red.
    #[error("red: {0}")]
    Network(String),
    #[error("autenticación: {0}")]
    Auth(String),
    #[error("clave de host rechazada: {0}")]
    HostKey(String),
    /// Sin algoritmos comunes con el servidor.
    #[error("negociación de algoritmos: {0}")]
    Negotiation(String),
    #[error("{0}")]
    Other(String),
}

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
    /// Sin 3DES, CBC, RC4, SHA-1, MD5, DH group1 ni DSA. Incluye strict KEX.
    pub fn secure() -> Self {
        todo!("friki-ssh: AlgorithmPolicy::secure")
    }

    /// `secure()` más los heredados, al final de cada lista.
    #[cfg(feature = "legacy")]
    pub fn legacy() -> Self {
        todo!("friki-ssh: AlgorithmPolicy::legacy")
    }

    /// `legacy()` solo si `cfg.legacy_algorithms` y la feature está activa.
    pub fn for_session(cfg: &SessionConfig) -> Self {
        let _ = cfg;
        todo!("friki-ssh: AlgorithmPolicy::for_session")
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
    _private: (),
}

impl Connection {
    pub async fn connect(
        cfg: &SessionConfig,
        verifier: Arc<dyn HostKeyVerifier>,
        prompter: Arc<dyn AuthPrompter>,
    ) -> Result<Self> {
        let _ = (cfg, verifier, prompter);
        todo!("friki-ssh: Connection::connect")
    }

    /// Canal interactivo con PTY.
    pub async fn open_shell(&self, term: &str, cols: u16, rows: u16) -> Result<(ShellWriter, ShellReader)> {
        let _ = (term, cols, rows);
        todo!("friki-ssh: Connection::open_shell")
    }

    pub async fn exec(&self, command: &str) -> Result<ExecOutput> {
        let _ = command;
        todo!("friki-ssh: Connection::exec")
    }

    pub async fn disconnect(self) -> Result<()> {
        todo!("friki-ssh: Connection::disconnect")
    }
}

/// Extremo de escritura; clonable para usarlo desde varias tareas.
#[derive(Clone)]
pub struct ShellWriter {
    _private: (),
}

impl ShellWriter {
    pub async fn write(&self, data: &[u8]) -> Result<()> {
        let _ = data;
        todo!("friki-ssh: ShellWriter::write")
    }

    pub async fn resize(&self, cols: u16, rows: u16) -> Result<()> {
        let _ = (cols, rows);
        todo!("friki-ssh: ShellWriter::resize")
    }
}

pub struct ShellReader {
    _private: (),
}

impl ShellReader {
    /// `None` cuando el canal ya está cerrado.
    pub async fn next_event(&mut self) -> Option<ShellEvent> {
        todo!("friki-ssh: ShellReader::next_event")
    }
}

/// Verificador por defecto basado en `friki_core::known_hosts`.
pub struct KnownHostsVerifier<F> {
    pub path: std::path::PathBuf,
    /// Se llama con clave desconocida o cambiada; con `Changed` solo
    /// debe poder devolver `Reject` o `AcceptOnce` tras aviso explícito.
    pub ask: F,
}

pub mod keys {
    use std::path::Path;

    /// Genera una clave Ed25519 en formato OpenSSH (privada en `path`, 0600;
    /// pública en `path.pub`) y devuelve la línea pública.
    pub fn generate_ed25519(path: &Path, comment: &str, passphrase: Option<&str>) -> super::Result<String> {
        let _ = (path, comment, passphrase);
        todo!("friki-ssh: keys::generate_ed25519")
    }
}
