//! Transferencia de ficheros por la sesión de terminal: Xmodem, Ymodem, Zmodem y Kermit (TODO.md §4).
//! Funciona sobre cualquier flujo de bytes; la TUI lo conecta al panel.
//!
//! Implementación propia y acotada. Limitaciones conocidas: Zmodem usa posiciones de 32 bits
//! (ficheros < 4 GiB) y ventanas de 8 KiB con ACK (sin streaming completo); Xmodem rellena el
//! último bloque con 0x1A, así que un fichero recibido por Xmodem pierde los 0x1A finales.

use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

mod kermit;
mod xy;
mod z;

#[cfg(test)]
mod tests;

/// Máximo de ficheros aceptados en un lote (Ymodem/Zmodem).
pub const MAX_FILES: usize = 1000;
/// Espera máxima por un byte/bloque antes de reintentar.
pub(crate) const T: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileProtocol {
    Xmodem,
    Ymodem,
    Zmodem,
    Kermit,
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
    const RQ: &[u8] = b"**\x18B00";
    const RI: &[u8] = b"**\x18B01";
    data.windows(RQ.len()).find_map(|w| {
        if w == RQ {
            Some(ZmodemStart::RemoteSends)
        } else if w == RI {
            Some(ZmodemStart::RemoteReceives)
        } else {
            None
        }
    })
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
) -> io::Result<Vec<PathBuf>> {
    // ponytail: sin tope práctico (u64::MAX); los llamadores con red hostil usan `receive_ex`
    Ok(receive_ex(io, protocol, dest_dir, progress, u64::MAX)
        .await?
        .files)
}

/// Resultado de `receive_ex`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Received {
    pub files: Vec<PathBuf>,
    /// Bytes leídos del flujo después del fin del protocolo (p. ej. el prompt del shell),
    /// que el llamador debe devolver al terminal.
    pub leftover: Vec<u8>,
}

/// `receive` con tope total de bytes escritos (corta y borra el parcial al superarlo, o al
/// superar el tamaño anunciado de un fichero) y devolución de los bytes sobrantes.
pub async fn receive_ex<T: AsyncRead + AsyncWrite + Unpin + Send>(
    io: &mut T,
    protocol: FileProtocol,
    dest_dir: &Path,
    progress: &mut (dyn FnMut(Progress) + Send),
    max_total_bytes: u64,
) -> std::io::Result<Received> {
    tokio::fs::create_dir_all(dest_dir).await?;
    let (files, leftover) = match protocol {
        FileProtocol::Xmodem => xy::receive(io, false, dest_dir, progress, max_total_bytes).await?,
        FileProtocol::Ymodem => xy::receive(io, true, dest_dir, progress, max_total_bytes).await?,
        FileProtocol::Zmodem => z::receive(io, dest_dir, progress, max_total_bytes).await?,
        FileProtocol::Kermit => {
            kermit::receive(io, dest_dir, progress, max_total_bytes, Default::default()).await?
        }
    };
    Ok(Received { files, leftover })
}

/// Error de tope superado; los receptores borran el fichero parcial al verlo.
pub(crate) fn too_large(msg: &str) -> io::Error {
    io::Error::new(io::ErrorKind::QuotaExceeded, msg.to_string())
}

/// Envía ficheros. Xmodem admite un único fichero.
pub async fn send<T: AsyncRead + AsyncWrite + Unpin + Send>(
    io: &mut T,
    protocol: FileProtocol,
    files: &[PathBuf],
    progress: &mut (dyn FnMut(Progress) + Send),
) -> io::Result<()> {
    if files.is_empty() || (protocol == FileProtocol::Xmodem && files.len() != 1) {
        return Err(invalid("número de ficheros no válido para el protocolo"));
    }
    if files.len() > MAX_FILES {
        return Err(invalid("demasiados ficheros"));
    }
    match protocol {
        FileProtocol::Xmodem => xy::send(io, false, files, progress).await,
        FileProtocol::Ymodem => xy::send(io, true, files, progress).await,
        FileProtocol::Zmodem => z::send(io, files, progress).await,
        FileProtocol::Kermit => kermit::send(io, files, progress, Default::default()).await,
    }
}

pub(crate) fn invalid(msg: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, msg.to_string())
}

pub(crate) fn aborted() -> io::Error {
    io::Error::new(io::ErrorKind::ConnectionAborted, "transferencia cancelada")
}

pub(crate) fn proto(msg: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg.to_string())
}

/// Flujo con búfer de lectura y timeout por byte.
pub(crate) struct Conn<'a, S> {
    io: &'a mut S,
    buf: Vec<u8>,
    pos: usize,
}

impl<'a, S: AsyncRead + AsyncWrite + Unpin + Send> Conn<'a, S> {
    pub(crate) fn new(io: &'a mut S) -> Self {
        Self {
            io,
            buf: Vec::new(),
            pos: 0,
        }
    }

    pub(crate) async fn byte(&mut self, t: Duration) -> io::Result<u8> {
        if self.pos >= self.buf.len() {
            self.buf.resize(4096, 0);
            self.pos = 0;
            let r = tokio::time::timeout(t, self.io.read(&mut self.buf)).await;
            let n = match r {
                Ok(r) => r?,
                Err(_) => {
                    self.buf.clear();
                    return Err(io::Error::new(io::ErrorKind::TimedOut, "timeout"));
                }
            };
            self.buf.truncate(n);
            if n == 0 {
                return Err(io::ErrorKind::UnexpectedEof.into());
            }
        }
        let b = self.buf[self.pos];
        self.pos += 1;
        Ok(b)
    }

    /// Bytes ya leídos del flujo y aún no consumidos (lo que sigue al fin del protocolo).
    pub(crate) fn rest(&mut self) -> Vec<u8> {
        let r = self.buf.get(self.pos..).unwrap_or_default().to_vec();
        self.buf.clear();
        self.pos = 0;
        r
    }

    /// Devuelve el último byte leído.
    pub(crate) fn unread(&mut self) {
        self.pos = self.pos.saturating_sub(1);
    }

    pub(crate) async fn write(&mut self, data: &[u8]) -> io::Result<()> {
        self.io.write_all(data).await?;
        self.io.flush().await
    }

    /// Cancelación de cortesía (ocho CAN y ocho BS); los errores se ignoran.
    pub(crate) async fn cancel(&mut self) {
        let _ = self.write(&[0x18; 8]).await;
        let _ = self.write(&[0x08; 8]).await;
    }
}

const fn crc16_table() -> [u16; 256] {
    let mut t = [0u16; 256];
    let mut i = 0;
    while i < 256 {
        let mut c = (i as u16) << 8;
        let mut k = 0;
        while k < 8 {
            c = if c & 0x8000 != 0 {
                (c << 1) ^ 0x1021
            } else {
                c << 1
            };
            k += 1;
        }
        t[i] = c;
        i += 1;
    }
    t
}

const fn crc32_table() -> [u32; 256] {
    let mut t = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut c = i as u32;
        let mut k = 0;
        while k < 8 {
            c = if c & 1 != 0 {
                (c >> 1) ^ 0xEDB8_8320
            } else {
                c >> 1
            };
            k += 1;
        }
        t[i] = c;
        i += 1;
    }
    t
}

static CRC16_T: [u16; 256] = crc16_table();
static CRC32_T: [u32; 256] = crc32_table();

/// CRC-16/XMODEM incremental (init 0).
pub(crate) fn crc16_update(mut crc: u16, data: &[u8]) -> u16 {
    for &b in data {
        crc = (crc << 8) ^ CRC16_T[usize::from((crc >> 8) as u8 ^ b)];
    }
    crc
}

/// CRC-32 IEEE incremental; empezar en `!0` y complementar al final.
pub(crate) fn crc32_update(mut crc: u32, data: &[u8]) -> u32 {
    for &b in data {
        crc = (crc >> 8) ^ CRC32_T[usize::from((crc as u8) ^ b)];
    }
    crc
}

fn bad_char(c: char) -> bool {
    c.is_control() || matches!(c, '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}')
}

/// Deja solo el último componente del nombre, sin controles, bidi, puntos iniciales ni
/// finales, con `:` sustituido y los nombres reservados de Windows prefijados.
pub(crate) fn sanitize_name(raw: &[u8]) -> String {
    let s = String::from_utf8_lossy(raw);
    let last = s.rsplit(['/', '\\']).next().unwrap_or("");
    let clean: String = last
        .chars()
        .filter(|&c| !bad_char(c))
        .map(|c| if c == ':' { '_' } else { c })
        .take(200)
        .collect();
    let clean = clean
        .trim_start_matches(['.', ' '])
        .trim_end_matches(['.', ' ']);
    let one_normal = {
        let mut it = Path::new(clean).components();
        matches!(
            (it.next(), it.next()),
            (Some(std::path::Component::Normal(_)), None)
        )
    };
    if clean.is_empty() || !one_normal {
        return "file".to_string();
    }
    let stem = clean.split('.').next().unwrap_or("").to_ascii_uppercase();
    let reserved = matches!(
        stem.as_str(),
        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
    ) || (stem.len() == 4
        && (stem.starts_with("COM") || stem.starts_with("LPT"))
        && stem.ends_with(|c: char| c.is_ascii_digit()));
    if reserved {
        format!("_{clean}")
    } else {
        clean.to_string()
    }
}

/// Crea `dir/nombre` sin sobrescribir: prueba `nombre`, `nombre.1`, `nombre.2`…
pub(crate) async fn create_unique(
    dir: &Path,
    name: &str,
) -> io::Result<(tokio::fs::File, PathBuf)> {
    for n in 0..10_000u32 {
        let candidate = if n == 0 {
            name.to_string()
        } else {
            format!("{name}.{n}")
        };
        let path = dir.join(candidate);
        match tokio::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .await
        {
            Ok(f) => return Ok((f, path)),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "no hay nombre libre",
    ))
}

/// Lee hasta `n` bytes (menos solo al final del fichero).
pub(crate) async fn read_chunk(f: &mut tokio::fs::File, n: usize) -> io::Result<Vec<u8>> {
    let mut v = vec![0u8; n];
    let mut got = 0;
    while got < n {
        let k = f.read(&mut v[got..]).await?;
        if k == 0 {
            break;
        }
        got += k;
    }
    v.truncate(got);
    Ok(v)
}

/// Nombre base de un fichero local para anunciarlo.
pub(crate) fn announce_name(p: &Path) -> String {
    let n = p
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    sanitize_name(n.as_bytes())
}
