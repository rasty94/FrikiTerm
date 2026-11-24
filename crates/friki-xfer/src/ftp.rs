//! FTP y FTPS explícito (AUTH TLS) con verificación de certificado obligatoria.

use crate::{RemoteFs, Result};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FtpOptions {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub tls: bool,
    /// Modo pasivo (por defecto).
    pub passive: bool,
}

pub async fn open(opts: &FtpOptions, password: &str) -> Result<Box<dyn RemoteFs>> {
    let _ = (opts, password);
    todo!("friki-xfer: ftp::open")
}
