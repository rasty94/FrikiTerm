//! Telnet (RFC 854 y opciones NAWS 1073, TTYPE 1091, SGA 858, ECHO 857, LFLOW 1372),
//! Telnet sobre TLS y TCP en crudo (TODO.md §4 y §9).

mod proto;

use std::io;
use std::sync::Arc;
use std::time::Duration;

use friki_core::transport::{Stream, StreamEvent, StreamReader, StreamWriter};
use proto::{Parser, Session};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, WriteHalf};
use tokio::net::TcpStream;
use tokio::sync::{Mutex, mpsc};
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

struct Inner<W> {
    w: WriteHalf<W>,
    /// `None` en modo RAW.
    session: Option<Session>,
}

struct Writer<S>(Arc<Mutex<Inner<S>>>);

#[async_trait::async_trait]
impl<S: AsyncRead + AsyncWrite + Send + 'static> StreamWriter for Writer<S> {
    async fn write(&self, data: &[u8]) -> io::Result<()> {
        let mut g = self.0.lock().await;
        let g = &mut *g;
        match &g.session {
            Some(s) => g.w.write_all(&s.encode(data)).await,
            None => g.w.write_all(data).await,
        }
    }

    async fn resize(&self, cols: u16, rows: u16) -> io::Result<()> {
        let mut g = self.0.lock().await;
        let g = &mut *g;
        match g.session.as_mut().and_then(|s| s.resize(cols, rows)) {
            Some(sb) => g.w.write_all(&sb).await,
            None => Ok(()),
        }
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
    let (mut r, w) = tokio::io::split(sock);
    let session = (!opts.raw).then(|| Session::new(&opts.term, cols, rows));
    let inner = Arc::new(Mutex::new(Inner { w, session }));
    let (tx, rx) = mpsc::channel(64);
    let shared = inner.clone();
    tokio::spawn(async move {
        let mut parser = Parser::default();
        let mut buf = vec![0u8; 8192];
        loop {
            let n = match r.read(&mut buf).await {
                Ok(0) => {
                    let _ = tx.send(StreamEvent::Closed).await;
                    return;
                }
                Ok(n) => n,
                Err(e) => {
                    let _ = tx.send(StreamEvent::Lost(e.to_string())).await;
                    return;
                }
            };
            let data = {
                let mut g = shared.lock().await;
                let g = &mut *g;
                match g.session.as_mut() {
                    None => buf[..n].to_vec(),
                    Some(s) => {
                        let (data, reply) = s.process(parser.feed(&buf[..n]));
                        if !reply.is_empty()
                            && let Err(e) = g.w.write_all(&reply).await
                        {
                            let _ = tx.send(StreamEvent::Lost(e.to_string())).await;
                            return;
                        }
                        data
                    }
                }
            };
            if !data.is_empty() && tx.send(StreamEvent::Data(data)).await.is_err() {
                return;
            }
        }
    });
    (Arc::new(Writer(inner)), Box::new(Reader(rx)))
}

#[cfg(test)]
mod tests;
