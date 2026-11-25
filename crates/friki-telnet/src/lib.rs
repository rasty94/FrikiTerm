//! Telnet (RFC 854 y opciones NAWS 1073, TTYPE 1091, SGA 858, ECHO 857, LFLOW 1372),
//! Telnet sobre TLS y TCP en crudo (TODO.md §4 y §9).

#[doc(hidden)]
pub mod proto; // pub solo para los objetivos de fuzzing

use std::io;
use std::sync::Arc;
use std::time::Duration;

use friki_core::transport::{Stream, StreamEvent, StreamReader, StreamWriter};
use proto::{Parser, Session};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio_rustls::TlsConnector;
use tokio_rustls::rustls::pki_types::ServerName;
use tokio_rustls::rustls::{ClientConfig, RootCertStore, crypto::ring};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TelnetOptions {
    /// TLS con verificación de certificado y nombre de host obligatoria (rustls).
    pub tls: bool,
    /// Sin negociación Telnet: bytes tal cual (sesión RAW).
    pub raw: bool,
    /// Tipo de terminal anunciado por TTYPE.
    pub term: String,
    pub connect_timeout_secs: u64,
}

/// Conecta y devuelve el flujo. `resize` envía NAWS si el servidor lo aceptó.
pub async fn connect(
    host: &str,
    port: u16,
    opts: &TelnetOptions,
    cols: u16,
    rows: u16,
) -> io::Result<Stream> {
    let roots = if opts.tls {
        Some(native_roots()?)
    } else {
        None
    };
    connect_with(host, port, opts, cols, rows, roots).await
}

fn native_roots() -> io::Result<RootCertStore> {
    let mut store = RootCertStore::empty();
    let loaded = rustls_native_certs::load_native_certs();
    store.add_parsable_certificates(loaded.certs);
    if store.is_empty() {
        return Err(io::Error::other("sin certificados raíz del sistema"));
    }
    Ok(store)
}

/// `roots` solo lo varían las pruebas; la verificación de nombre siempre está activa.
async fn connect_with(
    host: &str,
    port: u16,
    opts: &TelnetOptions,
    cols: u16,
    rows: u16,
    roots: Option<RootCertStore>,
) -> io::Result<Stream> {
    let limit = Duration::from_secs(opts.connect_timeout_secs.max(1));
    let tcp = tokio::time::timeout(limit, TcpStream::connect((host, port)))
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "tiempo de conexión agotado"))??;
    let _ = tcp.set_nodelay(true);
    match roots {
        None => Ok(start(tcp, opts, cols, rows)),
        Some(roots) => {
            let cfg = ClientConfig::builder_with_provider(Arc::new(ring::default_provider()))
                .with_safe_default_protocol_versions()
                .map_err(io::Error::other)?
                .with_root_certificates(roots)
                .with_no_client_auth();
            let name = ServerName::try_from(host.to_owned())
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
            let tls =
                tokio::time::timeout(limit, TlsConnector::from(Arc::new(cfg)).connect(name, tcp))
                    .await
                    .map_err(|_| {
                        io::Error::new(io::ErrorKind::TimedOut, "handshake TLS agotado")
                    })??;
            Ok(start(tls, opts, cols, rows))
        }
    }
}

/// `None` en modo RAW. Mutex síncrono: nunca se mantiene a través de un `await`.
type Shared = Arc<std::sync::Mutex<Option<Session>>>;

/// Las escrituras solo se encolan; una tarea propia las vuelca al socket (cola llena = `WouldBlock`).
struct Writer {
    session: Shared,
    queue: mpsc::Sender<Vec<u8>>,
}

fn poisoned() -> io::Error {
    io::Error::other("sesión Telnet envenenada")
}

impl Writer {
    fn push(&self, d: Vec<u8>) -> io::Result<()> {
        self.queue.try_send(d).map_err(|e| match e {
            mpsc::error::TrySendError::Full(_) => {
                io::Error::new(io::ErrorKind::WouldBlock, "cola de escritura llena")
            }
            mpsc::error::TrySendError::Closed(_) => {
                io::Error::new(io::ErrorKind::BrokenPipe, "conexión cerrada")
            }
        })
    }
}

#[async_trait::async_trait]
impl StreamWriter for Writer {
    async fn write(&self, data: &[u8]) -> io::Result<()> {
        let out = match self.session.lock().map_err(|_| poisoned())?.as_ref() {
            Some(s) => s.encode(data),
            None => data.to_vec(),
        };
        self.push(out)
    }

    async fn resize(&self, cols: u16, rows: u16) -> io::Result<()> {
        let sb = self
            .session
            .lock()
            .map_err(|_| poisoned())?
            .as_mut()
            .and_then(|s| s.resize(cols, rows));
        sb.map_or(Ok(()), |sb| self.push(sb))
    }
}

struct Reader(mpsc::Receiver<StreamEvent>);

#[async_trait::async_trait]
impl StreamReader for Reader {
    async fn next_event(&mut self) -> Option<StreamEvent> {
        self.0.recv().await
    }
}

fn start<S>(sock: S, opts: &TelnetOptions, cols: u16, rows: u16) -> Stream
where
    S: AsyncRead + AsyncWrite + Send + 'static,
{
    let (mut r, mut w) = tokio::io::split(sock);
    let session: Shared = Arc::new(std::sync::Mutex::new(
        (!opts.raw).then(|| Session::new(&opts.term, cols, rows)),
    ));
    let (wq, mut wrx) = mpsc::channel::<Vec<u8>>(256);
    // La lectora solo guarda un sender débil: al soltar el `Writer` la cola se cierra y
    // esta tarea termina, soltando la mitad de escritura.
    let replies = wq.downgrade();
    tokio::spawn(async move {
        while let Some(d) = wrx.recv().await {
            if w.write_all(&d).await.is_err() {
                return;
            }
        }
        let _ = w.shutdown().await;
    });
    let (tx, rx) = mpsc::channel(64);
    let shared = session.clone();
    tokio::spawn(async move {
        let mut parser = Parser::default();
        let mut buf = vec![0u8; 8192];
        loop {
            let n = tokio::select! {
                // Se soltó el lector: cerrar la mitad de lectura sin esperar datos.
                _ = tx.closed() => return,
                n = r.read(&mut buf) => match n {
                    Ok(0) => {
                        let _ = tx.send(StreamEvent::Closed).await;
                        return;
                    }
                    Ok(n) => n,
                    Err(e) => {
                        let _ = tx.send(StreamEvent::Lost(e.to_string())).await;
                        return;
                    }
                },
            };
            let (data, reply) = match shared.lock() {
                Ok(mut g) => match g.as_mut() {
                    None => (buf[..n].to_vec(), Vec::new()),
                    Some(s) => s.process(parser.feed(&buf[..n])),
                },
                Err(_) => return,
            };
            if !reply.is_empty()
                && let Some(q) = replies.upgrade()
                && q.send(reply).await.is_err()
            {
                let _ = tx.send(StreamEvent::Lost("conexión cerrada".into())).await;
                return;
            }
            if !data.is_empty() && tx.send(StreamEvent::Data(data)).await.is_err() {
                return;
            }
        }
    });
    (
        Arc::new(Writer { session, queue: wq }),
        Box::new(Reader(rx)),
    )
}

#[cfg(test)]
mod tests;
