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

/// Clasificación gruesa de un error (la CLI la usa para el código de salida).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    Network,
    NotFound,
    PermissionDenied,
    Protocol,
    Cancelled,
    Other,
}

impl Error {
    pub fn kind(&self) -> ErrorKind {
        use std::io::ErrorKind as K;
        match self {
            Error::NotFound(_) => ErrorKind::NotFound,
            Error::PermissionDenied(_) => ErrorKind::PermissionDenied,
            Error::Protocol(_) => ErrorKind::Protocol,
            Error::Cancelled => ErrorKind::Cancelled,
            Error::Ssh(_) => ErrorKind::Network,
            Error::Io(e) => match e.kind() {
                K::NotFound => ErrorKind::NotFound,
                K::PermissionDenied => ErrorKind::PermissionDenied,
                K::ConnectionReset
                | K::ConnectionAborted
                | K::ConnectionRefused
                | K::NotConnected
                | K::BrokenPipe
                | K::TimedOut
                | K::UnexpectedEof => ErrorKind::Network,
                _ => ErrorKind::Other,
            },
        }
    }
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
    /// Fija los permisos (bits `rwx`). Por defecto no hace nada (FTP, sistemas sin modos).
    async fn set_mode(&self, _path: &str, _mode: u32) -> Result<()> {
        Ok(())
    }
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

/// Comprueba que un nombre recibido del remoto es un único componente seguro: sin
/// separadores, `:`, controles (incluido NUL), `.`/`..`, vacío, acabado en `.` o espacio,
/// ni nombres reservados de Windows (`CON`, `NUL`, `COM1`..., con o sin extensión).
/// Lo usan descargas, sincronización y SCP.
pub fn safe_name(name: &str) -> bool {
    if name.is_empty()
        || name.ends_with(['.', ' '])
        || name
            .chars()
            .any(|c| c.is_control() || matches!(c, '/' | '\\' | ':'))
    {
        return false;
    }
    let stem = name.split('.').next().unwrap_or("").trim_end();
    let up = stem.to_ascii_uppercase();
    let reserved = matches!(up.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (up.len() == 4
            && (up.starts_with("COM") || up.starts_with("LPT"))
            && matches!(up.as_bytes()[3], b'1'..=b'9'));
    !reserved
        && matches!(
            Path::new(name).components().collect::<Vec<_>>().as_slice(),
            [std::path::Component::Normal(_)]
        )
}

/// Ruta local destino para un nombre remoto; `None` si el nombre no es seguro.
pub fn local_target(dir: &Path, name: &str) -> Option<std::path::PathBuf> {
    safe_name(name).then(|| dir.join(name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_name_rules() {
        for ok in ["a.txt", ".bashrc", "con_x", "COM10", "a b", "x.y.z"] {
            assert!(safe_name(ok), "{ok}");
        }
        for bad in [
            "", ".", "..", "a/b", "a\\b", "a:b", "a\nb", "a\0b", "\x1b[0m", "a.", "a ", "CON",
            "nul", "Aux.txt", "com1", "LPT9.log", "prn .x",
        ] {
            assert!(!safe_name(bad), "{bad:?}");
        }
    }

    #[test]
    fn error_kinds() {
        use std::io::ErrorKind as K;
        assert_eq!(Error::Io(K::TimedOut.into()).kind(), ErrorKind::Network);
        assert_eq!(Error::Io(K::NotFound.into()).kind(), ErrorKind::NotFound);
        assert_eq!(Error::Cancelled.kind(), ErrorKind::Cancelled);
        assert_eq!(Error::Protocol("x".into()).kind(), ErrorKind::Protocol);
    }
}
