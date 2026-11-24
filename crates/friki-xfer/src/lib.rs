//! Transferencia de ficheros (TODO.md §5): SFTP, SCP, FTP/FTPS, cola y sincronización.

use std::path::Path;

use tokio::io::{AsyncRead, AsyncWrite};

pub mod cache;
pub mod ftp;
pub mod local;
pub mod queue;
pub mod scp;
pub mod sftp;
pub mod sync;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("E/S: {0}")]
    Io(#[from] std::io::Error),
    #[error("no existe: {0}")]
    NotFound(String),
    #[error("permiso denegado: {0}")]
    PermissionDenied(String),
    #[error("protocolo: {0}")]
    Protocol(String),
    #[error("cancelado")]
    Cancelled,
    #[error(transparent)]
    Ssh(#[from] friki_ssh::Error),
}

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    File,
    Dir,
    Symlink,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    /// Ruta completa con `/` como separador.
    pub path: String,
    pub kind: Kind,
    pub size: u64,
    /// Segundos Unix.
    pub mtime: Option<i64>,
    pub mode: Option<u32>,
}

pub type Reader = Box<dyn AsyncRead + Send + Unpin>;
pub type Writer = Box<dyn AsyncWrite + Send + Unpin>;

/// Sistema de ficheros (remoto o local) sobre el que trabajan la cola y la sincronización.
#[async_trait::async_trait]
pub trait RemoteFs: Send + Sync {
    /// Entradas del directorio (sin `.` ni `..`).
    async fn list(&self, path: &str) -> Result<Vec<Entry>>;
    /// `None` si no existe. No sigue enlaces simbólicos.
    async fn stat(&self, path: &str) -> Result<Option<Entry>>;
    async fn mkdir(&self, path: &str) -> Result<()>;
    async fn remove_file(&self, path: &str) -> Result<()>;
    async fn remove_dir(&self, path: &str) -> Result<()>;
    async fn rename(&self, from: &str, to: &str) -> Result<()>;
    /// Lectura desde `offset` (reanudación).
    async fn open_read(&self, path: &str, offset: u64) -> Result<Reader>;
    /// `offset = 0` crea o trunca; `offset > 0` continúa (reanudación).
    async fn open_write(&self, path: &str, offset: u64) -> Result<Writer>;
    async fn set_mtime(&self, path: &str, mtime: i64) -> Result<()>;
    /// Nombre corto para mensajes (`sftp://user@host`).
    fn label(&self) -> String;
}

/// Une rutas con `/` sin duplicar separadores.
pub fn join(base: &str, name: &str) -> String {
    if base.is_empty() || base == "/" && name.starts_with('/') {
        return name.to_string();
    }
    format!(
        "{}/{}",
        base.trim_end_matches('/'),
        name.trim_start_matches('/')
    )
}

/// Comprueba que un nombre recibido del remoto es un único componente seguro
/// (sin `/`, `\\`, `..`, NUL ni vacío). Lo usan descargas y sincronización.
pub fn safe_name(name: &str) -> bool {
    !(name.is_empty() || name == "." || name == ".." || name.contains(['/', '\\', '\0']))
}

/// Ruta local destino para un nombre remoto; `None` si el nombre no es seguro.
pub fn local_target(dir: &Path, name: &str) -> Option<std::path::PathBuf> {
    safe_name(name).then(|| dir.join(name))
}
