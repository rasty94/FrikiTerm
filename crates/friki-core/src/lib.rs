//! Tipos compartidos: sesiones, `known_hosts`, secretos y rutas.
//! No depende de ningún otro crate del workspace.

pub mod config;
mod fsutil;
pub mod known_hosts;
pub mod paths;
pub mod secrets;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("E/S: {0}")]
    Io(#[from] std::io::Error),
    #[error("formato inválido en {path}: {msg}")]
    Parse { path: String, msg: String },
    #[error("llavero del sistema: {0}")]
    Secret(String),
}

pub type Result<T> = std::result::Result<T, Error>;
