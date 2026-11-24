//! Transferencia de ficheros por la sesión de terminal: Xmodem, Ymodem y Zmodem (TODO.md §4).
//! Funciona sobre cualquier flujo de bytes; la TUI lo conecta al panel.

use std::path::{Path, PathBuf};

use tokio::io::{AsyncRead, AsyncWrite};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileProtocol {
    Xmodem,
    Ymodem,
    Zmodem,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Progress {
    pub file: String,
    pub done: u64,
    pub total: Option<u64>,
}

/// Bytes recibidos del remoto que anuncian un envío Zmodem (`rz`/`sz`): `**\x18B00`
/// (ZRQINIT) o `**\x18B01` (ZRINIT). La TUI lo usa para ofrecer la transferencia.
pub fn detect(data: &[u8]) -> Option<ZmodemStart> {
    let _ = data;
    todo!("friki-zmodem: detect")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ZmodemStart {
    /// El remoto quiere enviar (`sz` en el remoto): hay que recibir.
    RemoteSends,
    /// El remoto está listo para recibir (`rz` en el remoto): hay que enviar.
    RemoteReceives,
}

/// Recibe ficheros en `dest_dir`. Nunca escribe fuera de `dest_dir` (nombres con `/`,
/// `..` o absolutos se sanean) ni sobrescribe un fichero existente (añade sufijo).
pub async fn receive<T: AsyncRead + AsyncWrite + Unpin + Send>(
    io: &mut T,
    protocol: FileProtocol,
    dest_dir: &Path,
    progress: &mut (dyn FnMut(Progress) + Send),
) -> std::io::Result<Vec<PathBuf>> {
    let _ = (io, protocol, dest_dir, progress);
    todo!("friki-zmodem: receive")
}

/// Envía ficheros. Xmodem admite un único fichero.
pub async fn send<T: AsyncRead + AsyncWrite + Unpin + Send>(
    io: &mut T,
    protocol: FileProtocol,
    files: &[PathBuf],
    progress: &mut (dyn FnMut(Progress) + Send),
) -> std::io::Result<()> {
    let _ = (io, protocol, files, progress);
    todo!("friki-zmodem: send")
}
