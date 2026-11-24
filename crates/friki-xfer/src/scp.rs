//! SCP (protocolo `scp -t` / `scp -f`) sobre `Connection::open_exec_stream`.
//! Validar todo lo que llega del remoto: nombres (`crate::safe_name`), tamaños y modos.

use std::path::Path;

use crate::Result;

/// Sube un fichero o, con `recursive`, un directorio.
pub async fn upload(
    conn: &friki_ssh::Connection,
    local: &Path,
    remote: &str,
    recursive: bool,
) -> Result<u64> {
    let _ = (conn, local, remote, recursive);
    todo!("friki-xfer: scp::upload")
}

/// Descarga en `local_dir`. Nunca escribe fuera de `local_dir`.
pub async fn download(
    conn: &friki_ssh::Connection,
    remote: &str,
    local_dir: &Path,
    recursive: bool,
) -> Result<u64> {
    let _ = (conn, remote, local_dir, recursive);
    todo!("friki-xfer: scp::download")
}
