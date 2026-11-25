//! Servidor TFTP mínimo: RFC 1350 + opciones RFC 2347/2348/2349 (`blksize`, `timeout`, `tsize`).
//! Solo modo octeto (netascii se trata como octeto). Todo nombre queda confinado a la raíz.

use std::io;
use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use friki_xfer::safe_name;
use tokio::fs::{self, File};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UdpSocket;
use tokio::sync::Semaphore;
use tokio::task::JoinSet;
use tokio::time::timeout;

const RETRIES: u32 = 5;
/// Transferencias simultáneas; las peticiones que sobran reciben ERROR "ocupado".
// ponytail: tope fijo; hacerlo configurable si hace falta más.
const MAX_TRANSFERS: usize = 32;
/// Tope del plazo que puede pedir el cliente (RFC 2349 admite hasta 255 s).
const MAX_TIMEOUT: u64 = 10;

/// Configuración del servidor.
pub struct Config {
    pub root: PathBuf,
    /// Admite WRQ (subidas). Por defecto, solo lectura.
    pub allow_write: bool,
    /// Permite que una subida sustituya un fichero existente.
    pub overwrite: bool,
    /// Tamaño máximo de una subida en bytes; al pasarlo se aborta y se borra el temporal.
    pub max_size: u64,
    /// Registro de cada petición y de su resultado.
    pub log: fn(&str),
}

/// Error TFTP: código del paquete ERROR y texto.
struct TErr(u16, String);

impl From<io::Error> for TErr {
    fn from(e: io::Error) -> Self {
        let code = match e.kind() {
            io::ErrorKind::NotFound => 1,
            io::ErrorKind::PermissionDenied => 2,
            io::ErrorKind::AlreadyExists => 6,
            _ => 0,
        };
        TErr(code, e.to_string())
    }
}

fn denied(m: &str) -> TErr {
    TErr(2, m.into())
}

/// Atiende peticiones hasta que se descarte el futuro (las transferencias en curso se abortan).
pub async fn serve(sock: UdpSocket, cfg: Config) -> io::Result<()> {
    let root = fs::canonicalize(&cfg.root).await?;
    let cfg = Arc::new(cfg);
    let ip = sock.local_addr()?.ip();
    let slots = Arc::new(Semaphore::new(MAX_TRANSFERS));
    let mut tasks = JoinSet::new();
    let mut buf = vec![0u8; 1024];
    loop {
        let (n, peer) = sock.recv_from(&mut buf).await?;
        while tasks.try_join_next().is_some() {}
        let Ok(permit) = slots.clone().try_acquire_owned() else {
            (cfg.log)(&format!("ocupado, rechazada petición de {peer}"));
            let _ = sock.send_to(&error_pkt(0, "servidor ocupado"), peer).await;
            continue;
        };
        let (req, root, cfg) = (buf[..n].to_vec(), root.clone(), cfg.clone());
        tasks.spawn(async move {
            let _permit = permit;
            handle(req, peer, ip, root, cfg).await;
        });
    }
}

async fn handle(req: Vec<u8>, peer: SocketAddr, ip: IpAddr, root: PathBuf, cfg: Arc<Config>) {
    let Ok(sock) = UdpSocket::bind((ip, 0)).await else {
        return;
    };
    if sock.connect(peer).await.is_err() {
        return;
    }
    let Some((op, name, opts)) = parse(&req) else {
        (cfg.log)(&format!("petición no válida de {peer}"));
        let _ = sock.send(&error_pkt(4, "petición no válida")).await;
        return;
    };
    let desc = format!("{} {name:?} de {peer}", if op == 1 { "RRQ" } else { "WRQ" });
    match transfer(op == 1, &name, opts, &sock, &root, &cfg).await {
        Ok(()) => (cfg.log)(&format!("{desc}: completada")),
        Err(TErr(code, msg)) => {
            (cfg.log)(&format!("{desc}: {msg}"));
            let _ = sock.send(&error_pkt(code, &msg)).await;
        }
    }
}

fn error_pkt(code: u16, msg: &str) -> Vec<u8> {
    let mut p = vec![0, 5, (code >> 8) as u8, code as u8];
    p.extend(msg.bytes().filter(|b| (0x20..0x7f).contains(b)));
    p.push(0);
    p
}

/// `(opcode, nombre, opciones)` de un RRQ/WRQ.
#[doc(hidden)]
pub type Req = (u16, String, Vec<(String, String)>);

/// Expuesta solo para fuzzing (`fuzz/`).
#[doc(hidden)]
pub fn parse(p: &[u8]) -> Option<Req> {
    let op = u16::from_be_bytes(p.get(..2)?.try_into().ok()?);
    if !matches!(op, 1 | 2) || p.last() != Some(&0) {
        return None;
    }
    let f: Vec<&[u8]> = p[2..p.len() - 1].split(|&b| b == 0).collect();
    let s = |b: &[u8]| String::from_utf8(b.to_vec()).ok();
    if f.len() < 2 || !f.len().is_multiple_of(2) {
        return None;
    }
    let opts = f[2..]
        .chunks(2)
        .map(|c| Some((s(c[0])?.to_ascii_lowercase(), s(c[1])?)))
        .collect::<Option<_>>()?;
    Some((op, s(f[0])?, opts))
}

/// Ruta dentro de la raíz: componentes seguros (sin `..`, absolutas ni `\`).
fn resolve(root: &Path, name: &str) -> Result<PathBuf, TErr> {
    if name.is_empty() || !name.split('/').all(safe_name) {
        return Err(denied("nombre no permitido"));
    }
    Ok(root.join(name))
}

/// Socket conectado con el cliente y reintentos.
struct Peer<'a> {
    sock: &'a UdpSocket,
    tmo: Duration,
    buf: Vec<u8>,
}

impl Peer<'_> {
    /// Envía `pkt` y espera un paquete que cumpla `want`, reenviando si vence el plazo.
    async fn exchange(&mut self, pkt: &[u8], want: impl Fn(&[u8]) -> bool) -> Result<usize, TErr> {
        for _ in 0..=RETRIES {
            self.sock.send(pkt).await?;
            while let Ok(r) = timeout(self.tmo, self.sock.recv(&mut self.buf)).await {
                let p = &self.buf[..r?];
                if p.starts_with(&[0, 5]) {
                    return Err(TErr(0, "el cliente abortó".into()));
                }
                if want(p) {
                    return Ok(p.len());
                }
            }
        }
        Err(TErr(0, "tiempo agotado".into()))
    }
}

fn block_pkt(op: u8, n: u16, data: &[u8]) -> Vec<u8> {
    let mut p = vec![0, op, (n >> 8) as u8, n as u8];
    p.extend_from_slice(data);
    p
}

fn is_pkt(p: &[u8], op: u8, n: u16) -> bool {
    p.len() >= 4 && p[..2] == [0, op] && p[2..4] == n.to_be_bytes()
}

/// Mismo fichero (dispositivo e inodo). Fuera de Unix solo cuenta la ruta canónica.
fn same_file(a: &std::fs::Metadata, b: &std::fs::Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        a.dev() == b.dev() && a.ino() == b.ino()
    }
    #[cfg(not(unix))]
    {
        let _ = (a, b);
        true
    }
}

async fn transfer(
    read: bool,
    name: &str,
    opts: Vec<(String, String)>,
    sock: &UdpSocket,
    root: &Path,
    cfg: &Config,
) -> Result<(), TErr> {
    let mut path = resolve(root, name)?;
    let mut blk = 512usize;
    let mut peer = Peer {
        sock,
        tmo: Duration::from_secs(3),
        buf: Vec::new(),
    };
    // Lectura: se abre primero y se comprueba el fichero abierto (mismo inodo que la ruta
    // canónica, dentro de la raíz). Escritura: destino y temporal se nombran desde el
    // directorio canónico ya confinado.
    let file;
    let mut size = 0;
    let tmp = if read {
        let f = File::open(&path).await?;
        let md = f.metadata().await?;
        let real = fs::canonicalize(&path).await?;
        if !real.starts_with(root) || !same_file(&md, &fs::metadata(&real).await?) {
            return Err(denied("fuera de la raíz"));
        }
        if !md.is_file() {
            return Err(denied("no es un fichero"));
        }
        size = md.len();
        file = f;
        None
    } else {
        if !cfg.allow_write {
            return Err(denied("escritura desactivada"));
        }
        let last = name.rsplit('/').next().unwrap_or("");
        let parent = path.parent().ok_or_else(|| denied("nombre no permitido"))?;
        let dir = fs::canonicalize(parent).await?;
        if !dir.starts_with(root) {
            return Err(denied("fuera de la raíz"));
        }
        path = dir.join(last);
        // ponytail: comprobar y renombrar no es atómico; basta para un servidor local.
        if !cfg.overwrite && fs::symlink_metadata(&path).await.is_ok() {
            return Err(TErr(6, "el fichero ya existe".into()));
        }
        let port = sock.local_addr()?.port();
        let tmp = dir.join(format!(".{last}.{port}.tftp"));
        file = File::options()
            .write(true)
            .create_new(true)
            .open(&tmp)
            .await?;
        Some(tmp)
    };

    let mut oack: Vec<(&str, String)> = Vec::new();
    for (k, v) in &opts {
        match (k.as_str(), v.parse::<u64>()) {
            ("blksize", Ok(n)) => {
                blk = n.clamp(8, 65464) as usize;
                oack.push(("blksize", blk.to_string()));
            }
            ("timeout", Ok(n @ 1..=255)) => {
                let n = n.min(MAX_TIMEOUT);
                peer.tmo = Duration::from_secs(n);
                oack.push(("timeout", n.to_string()));
            }
            ("tsize", Ok(n)) => oack.push(("tsize", if read { size } else { n }.to_string())),
            _ => {}
        }
    }
    peer.buf = vec![0; blk + 4];
    let oack_pkt = (!oack.is_empty()).then(|| {
        let mut p = vec![0, 6];
        for (k, v) in &oack {
            p.extend_from_slice(format!("{k}\0{v}\0").as_bytes());
        }
        p
    });

    let Some(tmp) = tmp else {
        return send_file(&mut peer, file, blk, oack_pkt).await;
    };
    // El ACK final sale tras el rename: al recibirlo, el cliente ya ve el fichero.
    let res = match recv_file(&mut peer, file, blk, oack_pkt, cfg.max_size).await {
        Ok(ack) => fs::rename(&tmp, &path)
            .await
            .map(|()| ack)
            .map_err(TErr::from),
        Err(e) => Err(e),
    };
    match res {
        Ok(ack) => Ok(sock.send(&ack).await.map(drop)?),
        Err(e) => {
            let _ = fs::remove_file(&tmp).await;
            Err(e)
        }
    }
}

async fn send_file(
    peer: &mut Peer<'_>,
    mut f: File,
    blk: usize,
    oack: Option<Vec<u8>>,
) -> Result<(), TErr> {
    if let Some(p) = oack {
        peer.exchange(&p, |p| is_pkt(p, 4, 0)).await?;
    }
    let mut n = 0u16;
    loop {
        let mut data = vec![0; blk];
        let mut len = 0;
        while len < blk {
            match f.read(&mut data[len..]).await? {
                0 => break,
                r => len += r,
            }
        }
        n = n.wrapping_add(1);
        let pkt = block_pkt(3, n, &data[..len]);
        peer.exchange(&pkt, |p| is_pkt(p, 4, n)).await?;
        if len < blk {
            return Ok(());
        }
    }
}

async fn recv_file(
    peer: &mut Peer<'_>,
    mut f: File,
    blk: usize,
    oack: Option<Vec<u8>>,
    max: u64,
) -> Result<Vec<u8>, TErr> {
    let mut pkt = oack.unwrap_or_else(|| block_pkt(4, 0, &[]));
    let mut n = 0u16;
    let mut total = 0u64;
    loop {
        n = n.wrapping_add(1);
        let len = peer.exchange(&pkt, |p| is_pkt(p, 3, n)).await?;
        total += (len - 4) as u64;
        if total > max {
            return Err(TErr(3, "tamaño máximo excedido".into()));
        }
        f.write_all(&peer.buf[4..len]).await?;
        pkt = block_pkt(4, n, &[]);
        if len - 4 < blk {
            f.flush().await?;
            return Ok(pkt);
        }
    }
}

#[cfg(test)]
mod tests;
