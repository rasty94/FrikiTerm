//! FTP y FTPS explícito (AUTH TLS) con verificación de certificado obligatoria.
//!
//! Una conexión de control (tras un `Mutex`) atiende los comandos de metadatos; cada
//! `open_read`/`open_write` abre su propia conexión, así las transferencias no bloquean
//! el resto de operaciones ni se pisan entre sí.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::{Duration, UNIX_EPOCH};

use suppaftp::list::{File, ListParser, PosixPexQuery};
use suppaftp::tokio::{AsyncRustlsConnector, ImplAsyncFtpStream, TransferStream};
use suppaftp::types::FileType as TransferType;
use suppaftp::{FtpError, Status};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::sync::Mutex;
use tokio_rustls::TlsConnector;
use tokio_rustls::rustls::{ClientConfig, RootCertStore};

use crate::{Entry, Error, Kind, Reader, RemoteFs, Result, Writer, join, safe_name};

type Stream = ImplAsyncFtpStream<suppaftp::tokio::AsyncRustlsStream>;
type Transfer = TransferStream<suppaftp::tokio::AsyncRustlsStream>;

const TIMEOUT: Duration = Duration::from_secs(20);

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
    let mut ctl = connect(opts, password).await?;
    // MLSD solo si el servidor lo anuncia en FEAT (MLST); si no, LIST.
    let mlsd = ctl
        .feat()
        .await
        .map(|f| f.keys().any(|k| k.eq_ignore_ascii_case("MLST")))
        .unwrap_or(false);
    Ok(Box::new(Ftp {
        opts: opts.clone(),
        password: friki_ssh::Zeroizing::new(password.to_string()),
        ctl: Mutex::new(ctl),
        mlsd,
    }))
}

/// TLS con raíces del sistema; rustls verifica cadena y nombre de host (sin excepciones).
fn tls_connector() -> Result<TlsConnector> {
    let mut roots = RootCertStore::empty();
    let native = rustls_native_certs::load_native_certs();
    let (added, _) = roots.add_parsable_certificates(native.certs);
    if added == 0 {
        return Err(Error::Protocol(
            "no hay certificados raíz del sistema para verificar FTPS".into(),
        ));
    }
    let provider = Arc::new(tokio_rustls::rustls::crypto::aws_lc_rs::default_provider());
    let cfg = ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| Error::Protocol(e.to_string()))?
        .with_root_certificates(roots)
        .with_no_client_auth();
    Ok(TlsConnector::from(Arc::new(cfg)))
}

async fn connect(opts: &FtpOptions, password: &str) -> Result<Stream> {
    let addr = format!("{}:{}", opts.host, opts.port);
    let mut s = tokio::time::timeout(TIMEOUT, Stream::connect(addr))
        .await
        .map_err(|_| Error::Io(std::io::ErrorKind::TimedOut.into()))?
        .map_err(map_err)?;
    if opts.tls {
        s = s
            .into_secure(AsyncRustlsConnector::from(tls_connector()?), &opts.host)
            .await
            .map_err(map_err)?;
    }
    s.login(opts.user.as_str(), password)
        .await
        .map_err(map_err)?;
    s.transfer_type(TransferType::Binary)
        .await
        .map_err(map_err)?;
    if opts.passive {
        // Conectar a la IP del servidor de control, no a la que dicte la respuesta PASV.
        s.set_passive_nat_workaround(true);
    } else {
        s = s.active_mode(TIMEOUT);
    }
    Ok(s)
}

struct Ftp {
    opts: FtpOptions,
    password: friki_ssh::Zeroizing<String>,
    ctl: Mutex<Stream>,
    mlsd: bool,
}

fn map_err(e: FtpError) -> Error {
    match e {
        FtpError::UnexpectedResponse(r) => {
            let body = String::from_utf8_lossy(&r.body).trim().to_string();
            match r.status {
                Status::NotLoggedIn => Error::PermissionDenied(body),
                Status::FileUnavailable => {
                    let l = body.to_ascii_lowercase();
                    if l.contains("denied") || l.contains("permission") {
                        Error::PermissionDenied(body)
                    } else {
                        Error::NotFound(body)
                    }
                }
                s => Error::Protocol(format!("{}: {body}", s.code())),
            }
        }
        FtpError::ConnectionError(e) => Error::Io(e),
        e => Error::Protocol(e.to_string()),
    }
}

/// CR/LF en una ruta permitiría inyectar comandos FTP.
fn check(path: &str) -> Result<()> {
    if path.contains(['\r', '\n', '\0']) {
        return Err(Error::Protocol(format!("ruta no válida: {path:?}")));
    }
    Ok(())
}

/// Convierte líneas de `MLSD` (o `LIST` estilo Unix/DOS) en entradas.
/// Las líneas que no se entienden (`total N`, líneas vacías) se omiten, igual que los
/// nombres inseguros (`..`, con `/`).
pub fn parse_listing(dir: &str, lines: &[String], mlsd: bool) -> Vec<Entry> {
    lines
        .iter()
        .filter_map(|l| {
            let f = if mlsd {
                ListParser::parse_mlsd(l)
            } else {
                ListParser::parse_posix(l).or_else(|_| ListParser::parse_dos(l))
            }
            .ok()?;
            safe_name(f.name()).then(|| to_entry(dir, &f))
        })
        .collect()
}

fn to_entry(dir: &str, f: &File) -> Entry {
    let bit = |q, r, w, x| {
        (u32::from(f.can_read(q)) * r)
            | (u32::from(f.can_write(q)) * w)
            | (u32::from(f.can_execute(q)) * x)
    };
    let mode = bit(PosixPexQuery::Owner, 0o400, 0o200, 0o100)
        | bit(PosixPexQuery::Group, 0o040, 0o020, 0o010)
        | bit(PosixPexQuery::Others, 0o004, 0o002, 0o001);
    Entry {
        name: f.name().to_string(),
        path: join(dir, f.name()),
        kind: if f.is_directory() {
            Kind::Dir
        } else if f.is_symlink() {
            Kind::Symlink
        } else if f.is_file() {
            Kind::File
        } else {
            Kind::Other
        },
        size: f.size() as u64,
        mtime: f
            .modified()
            .duration_since(UNIX_EPOCH)
            .ok()
            .and_then(|d| i64::try_from(d.as_secs()).ok()),
        mode: Some(mode),
    }
}

/// `YYYYMMDDHHMMSS` en UTC para `MFMT`.
fn mfmt_time(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    // Fecha civil desde días (algoritmo de Hinnant).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}{m:02}{d:02}{:02}{:02}{:02}",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

#[async_trait::async_trait]
impl RemoteFs for Ftp {
    async fn list(&self, path: &str) -> Result<Vec<Entry>> {
        check(path)?;
        let mut c = self.ctl.lock().await;
        let lines = if self.mlsd {
            c.mlsd(Some(path)).await
        } else {
            c.list(Some(path)).await
        }
        .map_err(map_err)?;
        Ok(parse_listing(path, &lines, self.mlsd))
    }

    async fn stat(&self, path: &str) -> Result<Option<Entry>> {
        check(path)?;
        let trimmed = path.trim_end_matches('/');
        let Some((parent, name)) = trimmed.rsplit_once('/') else {
            return Ok(None);
        };
        if name.is_empty() {
            return Ok(None);
        }
        let parent = if parent.is_empty() { "/" } else { parent };
        match self.list(parent).await {
            Ok(v) => Ok(v.into_iter().find(|e| e.name == name)),
            Err(Error::NotFound(_)) => Ok(None),
            Err(e) => Err(e),
        }
    }

    async fn mkdir(&self, path: &str) -> Result<()> {
        check(path)?;
        self.ctl.lock().await.mkdir(path).await.map_err(map_err)
    }

    async fn remove_file(&self, path: &str) -> Result<()> {
        check(path)?;
        self.ctl.lock().await.rm(path).await.map_err(map_err)
    }

    async fn remove_dir(&self, path: &str) -> Result<()> {
        check(path)?;
        self.ctl.lock().await.rmdir(path).await.map_err(map_err)
    }

    async fn rename(&self, from: &str, to: &str) -> Result<()> {
        check(from)?;
        check(to)?;
        self.ctl
            .lock()
            .await
            .rename(from, to)
            .await
            .map_err(map_err)
    }

    async fn open_read(&self, path: &str, offset: u64) -> Result<Reader> {
        check(path)?;
        let mut c = connect(&self.opts, &self.password).await?;
        c.resume_transfer(usize::try_from(offset).map_err(|_| Error::Protocol("offset".into()))?)
            .await
            .map_err(map_err)?;
        let t = c.retr_as_stream(path).await.map_err(map_err)?;
        Ok(Box::new(Rd {
            _c: c,
            t: Some(t),
            fin: None,
        }))
    }

    async fn open_write(&self, path: &str, offset: u64) -> Result<Writer> {
        check(path)?;
        let mut c = connect(&self.opts, &self.password).await?;
        c.resume_transfer(usize::try_from(offset).map_err(|_| Error::Protocol("offset".into()))?)
            .await
            .map_err(map_err)?;
        let t = c.put_with_stream(path).await.map_err(map_err)?;
        Ok(Box::new(Wr {
            _c: c,
            t: Some(t),
            fin: None,
        }))
    }

    async fn set_mtime(&self, path: &str, mtime: i64) -> Result<()> {
        check(path)?;
        // MFMT (RFC 3659 borrador); si el servidor no lo soporta es un error `Protocol`.
        self.ctl
            .lock()
            .await
            .custom_command(format!("MFMT {} {path}", mfmt_time(mtime)), &[Status::File])
            .await
            .map(|_| ())
            .map_err(map_err)
    }

    fn label(&self) -> String {
        let scheme = if self.opts.tls { "ftps" } else { "ftp" };
        format!("{scheme}://{}@{}", self.opts.user, self.opts.host)
    }
}

type Fin = Pin<Box<dyn Future<Output = std::result::Result<(), FtpError>> + Send>>;

/// Lector: conserva el cliente (conexión de control) mientras dura la transferencia. Al
/// llegar a EOF espera la respuesta final: un 426 (aborto del servidor) es un error, no un
/// fichero truncado dado por bueno.
struct Rd {
    _c: Stream,
    t: Option<Transfer>,
    fin: Option<Fin>,
}

impl AsyncRead for Rd {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        if self.fin.is_none() {
            let Some(t) = self.t.as_mut() else {
                return Poll::Ready(Ok(()));
            };
            let before = buf.filled().len();
            match Pin::new(t).poll_read(cx, buf) {
                Poll::Ready(Ok(())) if buf.filled().len() == before && buf.remaining() > 0 => {}
                r => return r,
            }
            if let Some(t) = self.t.take() {
                self.fin = Some(Box::pin(t.finish()));
            }
        }
        match self.fin.as_mut() {
            Some(f) => match f.as_mut().poll(cx) {
                Poll::Pending => Poll::Pending,
                Poll::Ready(r) => {
                    self.fin = None;
                    Poll::Ready(r.map_err(std::io::Error::other))
                }
            },
            None => Poll::Ready(Ok(())),
        }
    }
}

/// Escritor: `shutdown` cierra la conexión de datos y espera la respuesta final (226).
struct Wr {
    _c: Stream,
    t: Option<Transfer>,
    fin: Option<Fin>,
}

impl AsyncWrite for Wr {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        match self.t.as_mut() {
            Some(t) => Pin::new(t).poll_write(cx, buf),
            None => Poll::Ready(Err(std::io::ErrorKind::BrokenPipe.into())),
        }
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match self.t.as_mut() {
            Some(t) => Pin::new(t).poll_flush(cx),
            None => Poll::Ready(Ok(())),
        }
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        if self.fin.is_none() {
            let Some(t) = self.t.as_mut() else {
                return Poll::Ready(Ok(()));
            };
            match Pin::new(t).poll_shutdown(cx) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
                Poll::Ready(Ok(())) => {}
            }
            if let Some(t) = self.t.take() {
                self.fin = Some(Box::pin(t.finish()));
            }
        }
        match self.fin.as_mut() {
            Some(f) => f.as_mut().poll(cx).map_err(std::io::Error::other),
            None => Poll::Ready(Ok(())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn parses_unix_list() {
        let l = lines(&[
            "total 12",
            "drwxr-xr-x    2 ftp      ftp          4096 Oct 19  2020 docs",
            "-rw-r--r--    1 ftp      ftp          1234 Oct 19  2020 a b.txt",
            "lrwxrwxrwx    1 ftp      ftp             5 Oct 19  2020 ln -> a b.txt",
            "-rw-r--r--    1 ftp      ftp             1 Oct 19  2020 ..",
            "",
        ]);
        let v = parse_listing("/pub", &l, false);
        assert_eq!(v.len(), 3, "{v:?}");
        assert_eq!(v[0].kind, Kind::Dir);
        assert_eq!(v[0].path, "/pub/docs");
        assert_eq!(v[0].mode, Some(0o755));
        assert_eq!(
            (v[1].name.as_str(), v[1].size, v[1].kind),
            ("a b.txt", 1234, Kind::File)
        );
        assert_eq!(v[1].mode, Some(0o644));
        assert_eq!(v[2].kind, Kind::Symlink);
        assert_eq!(v[2].name, "ln");
    }

    #[test]
    fn parses_mlsd_and_dos() {
        let l = lines(&[
            "type=dir;modify=20201019151930;UNIX.mode=0755; sub",
            "type=file;size=42;modify=20201019151930; f.txt",
            "type=cdir;modify=20201019151930; .",
            "type=pdir;modify=20201019151930; ..",
        ]);
        let v = parse_listing("/", &l, true);
        assert_eq!(
            v.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(),
            ["sub", "f.txt"]
        );
        assert_eq!(v[1].size, 42);
        assert_eq!(v[1].mtime, Some(1_603_120_770));
        let d = parse_listing(
            "/",
            &lines(&["10-19-20  03:19PM <DIR> pub", "10-19-20  03:19PM 12 x.txt"]),
            false,
        );
        assert_eq!(d.len(), 2, "{d:?}");
        assert_eq!(d[0].kind, Kind::Dir);
    }

    #[test]
    fn mfmt_and_check() {
        assert_eq!(mfmt_time(0), "19700101000000");
        assert_eq!(mfmt_time(1_603_120_770), "20201019151930");
        assert_eq!(mfmt_time(951_782_400), "20000229000000");
        assert!(check("a\r\nDELE x").is_err());
        assert!(check("/ok/path").is_ok());
    }
}
