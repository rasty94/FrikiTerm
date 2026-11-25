//! HTTP/S de solo lectura (descarga, `stat` por HEAD y reanudación con `Range`).
//!
//! TLS con rustls (mínimo 1.2, verificación de nombre siempre). Redirecciones limitadas y
//! nunca de https a http. Sin listado de directorios (HTML no es un protocolo).
use std::time::{Duration, UNIX_EPOCH};

use futures_util::TryStreamExt;
use reqwest::{Client, StatusCode, Url, redirect};

use crate::{Entry, Error, Kind, Reader, RemoteFs, Result, Writer};

const MAX_REDIRECTS: usize = 5;

/// Cliente reqwest común (HTTP y S3). `follow = false` desactiva las redirecciones.
pub(crate) fn client(follow: bool) -> Result<Client> {
    let policy = if follow {
        redirect::Policy::custom(|a| {
            if a.previous().len() >= MAX_REDIRECTS {
                a.error("demasiadas redirecciones")
            } else if a.previous().last().is_some_and(|p| p.scheme() == "https")
                && a.url().scheme() != "https"
            {
                a.error("redirección de https a http rechazada")
            } else {
                a.follow()
            }
        })
    } else {
        redirect::Policy::none()
    };
    Client::builder()
        .min_tls_version(reqwest::tls::Version::TLS_1_2)
        .redirect(policy)
        .connect_timeout(Duration::from_secs(20))
        .read_timeout(Duration::from_secs(60))
        .build()
        .map_err(map_err)
}

pub(crate) fn map_err(e: reqwest::Error) -> Error {
    use std::io::ErrorKind as K;
    let e = e.without_url();
    if e.is_timeout() {
        Error::Io(K::TimedOut.into())
    } else if e.is_connect() {
        Error::Io(K::ConnectionRefused.into())
    } else {
        Error::Protocol(e.to_string())
    }
}

pub(crate) fn map_status(s: StatusCode, what: &str) -> Error {
    match s {
        StatusCode::NOT_FOUND | StatusCode::GONE => Error::NotFound(what.to_string()),
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => {
            Error::PermissionDenied(format!("{what}: HTTP {s}"))
        }
        _ => Error::Protocol(format!("{what}: HTTP {s}")),
    }
}

pub(crate) fn unsupported(what: &str) -> Error {
    Error::Protocol(format!("{what} no está soportado por este backend"))
}

/// Cuerpo de una respuesta como lector asíncrono.
pub(crate) fn body_reader(r: reqwest::Response) -> Reader {
    let s = r
        .bytes_stream()
        .map_err(|e| std::io::Error::other(e.without_url().to_string()));
    Box::new(tokio_util::io::StreamReader::new(s))
}

/// `Content-Length` de la cabecera (en HEAD `content_length()` da 0: el cuerpo está vacío).
pub(crate) fn content_len(r: &reqwest::Response) -> u64 {
    r.headers()
        .get(reqwest::header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse().ok())
        .unwrap_or(0)
}

/// Segundos Unix de una cabecera `Last-Modified`.
pub(crate) fn last_modified(r: &reqwest::Response) -> Option<i64> {
    let v = r
        .headers()
        .get(reqwest::header::LAST_MODIFIED)?
        .to_str()
        .ok()?;
    let t = httpdate::parse_http_date(v).ok()?;
    i64::try_from(t.duration_since(UNIX_EPOCH).ok()?.as_secs()).ok()
}

pub struct HttpFs {
    client: Client,
    base: Url,
}

impl HttpFs {
    /// `origin`: `http(s)://host[:puerto]`; solo cuenta el origen, la ruta va en cada llamada.
    pub fn new(origin: &str) -> Result<Self> {
        let base = Url::parse(origin).map_err(|e| Error::Protocol(format!("URL: {e}")))?;
        if !matches!(base.scheme(), "http" | "https") || base.host_str().is_none() {
            return Err(Error::Protocol("se esperaba http:// o https://".into()));
        }
        Ok(Self {
            client: client(true)?,
            base,
        })
    }

    /// El origen queda fijo: una ruta como `//otro/x` no puede cambiar de host.
    fn url(&self, path: &str) -> Url {
        let (p, q) = path
            .split_once('?')
            .map_or((path, None), |(p, q)| (p, Some(q)));
        let mut u = self.base.clone();
        u.set_path(&format!("/{}", p.trim_start_matches('/')));
        u.set_query(q);
        u
    }
}

const RO: &str = "escribir por HTTP (solo lectura)";

#[async_trait::async_trait]
impl RemoteFs for HttpFs {
    async fn list(&self, _path: &str) -> Result<Vec<Entry>> {
        Err(unsupported("listar directorios por HTTP"))
    }

    async fn stat(&self, path: &str) -> Result<Option<Entry>> {
        // ponytail: servidores sin HEAD (405) dan error; añadir GET Range 0-0 si hace falta.
        let r = self
            .client
            .head(self.url(path))
            .send()
            .await
            .map_err(map_err)?;
        if matches!(r.status(), StatusCode::NOT_FOUND | StatusCode::GONE) {
            return Ok(None);
        }
        if !r.status().is_success() {
            return Err(map_status(r.status(), path));
        }
        let name = path.split('?').next().unwrap_or("");
        let name = name.trim_end_matches('/').rsplit('/').next().unwrap_or("");
        Ok(Some(Entry {
            name: name.to_string(),
            path: path.to_string(),
            kind: Kind::File,
            size: content_len(&r),
            mtime: last_modified(&r),
            mode: None,
        }))
    }

    async fn mkdir(&self, _: &str) -> Result<()> {
        Err(unsupported(RO))
    }
    async fn remove_file(&self, _: &str) -> Result<()> {
        Err(unsupported(RO))
    }
    async fn remove_dir(&self, _: &str) -> Result<()> {
        Err(unsupported(RO))
    }
    async fn rename(&self, _: &str, _: &str) -> Result<()> {
        Err(unsupported(RO))
    }
    async fn open_write(&self, _: &str, _: u64) -> Result<Writer> {
        Err(unsupported(RO))
    }
    async fn set_mtime(&self, _: &str, _: i64) -> Result<()> {
        Err(unsupported(RO))
    }

    async fn open_read(&self, path: &str, offset: u64) -> Result<Reader> {
        let mut req = self.client.get(self.url(path));
        if offset > 0 {
            req = req.header(reqwest::header::RANGE, format!("bytes={offset}-"));
        }
        let r = req.send().await.map_err(map_err)?;
        let ok = if offset > 0 {
            StatusCode::PARTIAL_CONTENT
        } else {
            StatusCode::OK
        };
        if r.status() != ok {
            // 200 con Range: el servidor lo ignora; seguir corrompería el fichero.
            return Err(if r.status().is_success() {
                Error::Protocol("el servidor no admite reanudar (Range)".into())
            } else {
                map_status(r.status(), path)
            });
        }
        Ok(body_reader(r))
    }

    fn label(&self) -> String {
        self.base.origin().ascii_serialization()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    const DATA: &[u8] = b"0123456789abcdef";

    /// Servidor HTTP/1.1 mínimo: /f (con Range), /r (redirige a /f), resto 404.
    async fn mock() -> String {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let Ok((mut s, _)) = l.accept().await else {
                    return;
                };
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 4096];
                    let n = s.read(&mut buf).await.unwrap_or(0);
                    let req = String::from_utf8_lossy(&buf[..n]).to_string();
                    let (line, _) = req.split_once("\r\n").unwrap_or(("", ""));
                    let mut it = line.split(' ');
                    let (method, path) = (it.next().unwrap_or(""), it.next().unwrap_or(""));
                    let range = req
                        .lines()
                        .find_map(|l| {
                            l.to_ascii_lowercase()
                                .strip_prefix("range: bytes=")
                                .map(str::to_string)
                        })
                        .and_then(|r| r.trim_end_matches('-').parse::<usize>().ok());
                    let (head, body): (String, &[u8]) = match path {
                        "/f" => match range {
                            Some(o) => (
                                format!(
                                    "HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\n",
                                    DATA.len() - o
                                ),
                                &DATA[o..],
                            ),
                            None => (
                                format!(
                                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nLast-Modified: Thu, 01 Jan 2026 00:00:00 GMT\r\n",
                                    DATA.len()
                                ),
                                DATA,
                            ),
                        },
                        "/r" => (
                            "HTTP/1.1 302 Found\r\nLocation: /f\r\nContent-Length: 0\r\n".into(),
                            b"",
                        ),
                        _ => (
                            "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n".into(),
                            b"",
                        ),
                    };
                    let body = if method == "HEAD" { &[][..] } else { body };
                    let _ = s
                        .write_all(format!("{head}Connection: close\r\n\r\n").as_bytes())
                        .await;
                    let _ = s.write_all(body).await;
                });
            }
        });
        format!("http://{addr}")
    }

    async fn read_all(fs: &HttpFs, p: &str, off: u64) -> Result<Vec<u8>> {
        let mut v = Vec::new();
        fs.open_read(p, off).await?.read_to_end(&mut v).await?;
        Ok(v)
    }

    #[tokio::test]
    async fn stat_get_range_y_errores() {
        let fs = HttpFs::new(&mock().await).unwrap();
        let e = fs.stat("/f").await.unwrap().unwrap();
        assert_eq!(
            (e.size, e.mtime, e.name.as_str()),
            (16, Some(1_767_225_600), "f")
        );
        assert!(fs.stat("/nope").await.unwrap().is_none());
        assert_eq!(read_all(&fs, "/f", 0).await.unwrap(), DATA);
        assert_eq!(read_all(&fs, "/f", 10).await.unwrap(), &DATA[10..]);
        // Redirección dentro de http: se sigue.
        assert_eq!(read_all(&fs, "/r", 0).await.unwrap(), DATA);
        assert!(matches!(
            read_all(&fs, "/nope", 0).await,
            Err(Error::NotFound(_))
        ));
        // Solo lectura.
        assert!(fs.open_write("/f", 0).await.is_err());
        assert!(fs.remove_file("/f").await.is_err());
        // Ruta con `//` no cambia de host.
        assert_eq!(fs.url("//evil.test/x").host_str(), Some("127.0.0.1"));
    }
}
