//! S3 y compatibles (MinIO, R2, ...): SigV4 con `rusty-s3` (URL prefirmadas) y `reqwest`+rustls.
//!
//! Las credenciales vienen del entorno (`AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`,
//! `AWS_SESSION_TOKEN`) o de un perfil de `~/.aws/credentials`; nunca se guardan en la
//! configuración de friki. Los directorios son prefijos: `mkdir`/`remove_dir` no hacen nada.
//!
//! Escritura: multipart directo al destino (`atomic_writes`): un objeto solo aparece al
//! completar la subida, así que un fallo no deja el destino a medias y no hay `.friki-part`
//! ni CopyObject. `rename` (CopyObject + DeleteObject) queda para usos explícitos.
use std::sync::Arc;
use std::time::Duration;

use reqwest::{Client, Method, RequestBuilder, Response, Url};
use rusty_s3::actions::{
    AbortMultipartUpload, CompleteMultipartUpload, CompleteMultipartUploadResponse,
    CreateMultipartUpload, DeleteObject, GetObject, HeadObject, ListObjectsV2, PutObject,
    UploadPart,
};
use rusty_s3::{Bucket, Credentials, S3Action, UrlStyle};
use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio::io::AsyncWrite;
use tokio::sync::mpsc;
use tokio_util::sync::PollSender;

use crate::http::{
    body_reader, client, content_len, last_modified, map_err, map_status, unsupported,
};
use crate::queue::PART_SUFFIX;
use crate::{Entry, Error, Kind, Reader, RemoteFs, Result, Writer, join, safe_name};

const TTL: Duration = Duration::from_secs(3600);
/// Tamaño de parte multipart (mínimo de S3: 5 MiB; máximo 10 000 partes => ~160 GB por objeto).
const PART: usize = 16 << 20;
/// Límite de un CopyObject único (el rename de `.friki-part`).
const COPY_MAX: u64 = 5 << 30;

#[derive(Debug, Clone)]
pub struct S3Options {
    pub bucket: String,
    pub region: String,
    /// `None`: AWS (`https://s3.<región>.amazonaws.com`).
    pub endpoint: Option<String>,
    /// Fuerza `https://host/bucket/clave` (siempre en endpoints propios).
    pub path_style: bool,
}

impl S3Options {
    /// Región y endpoint del entorno: `AWS_REGION`/`AWS_DEFAULT_REGION`,
    /// `FRIKI_S3_ENDPOINT`/`AWS_ENDPOINT_URL_S3`/`AWS_ENDPOINT_URL`, `FRIKI_S3_PATH_STYLE=1`.
    pub fn from_env(bucket: &str) -> Self {
        let var = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
        Self {
            bucket: bucket.to_string(),
            region: var("AWS_REGION")
                .or_else(|| var("AWS_DEFAULT_REGION"))
                .unwrap_or_else(|| "us-east-1".into()),
            endpoint: var("FRIKI_S3_ENDPOINT")
                .or_else(|| var("AWS_ENDPOINT_URL_S3"))
                .or_else(|| var("AWS_ENDPOINT_URL")),
            path_style: var("FRIKI_S3_PATH_STYLE").is_some_and(|v| v == "1"),
        }
    }
}

/// Busca `[perfil]` en un fichero tipo ini de AWS: (clave, secreto, token).
fn parse_profile(text: &str, profile: &str) -> Option<(String, String, Option<String>)> {
    let (mut on, mut k, mut s, mut t) = (false, None, None, None);
    for l in text.lines().map(str::trim) {
        if let Some(h) = l.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            on = h.trim() == profile;
        } else if on && let Some((n, v)) = l.split_once('=') {
            let v = Some(v.trim().to_string());
            match n.trim() {
                "aws_access_key_id" => k = v,
                "aws_secret_access_key" => s = v,
                "aws_session_token" => t = v,
                _ => {}
            }
        }
    }
    Some((k?, s?, t))
}

/// Credenciales del entorno o del perfil (`AWS_PROFILE`, por defecto `default`).
pub fn credentials_from_env() -> Result<Credentials> {
    let var = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
    let found = if let (Some(k), Some(s)) = (var("AWS_ACCESS_KEY_ID"), var("AWS_SECRET_ACCESS_KEY"))
    {
        Some((k, s, var("AWS_SESSION_TOKEN")))
    } else {
        let file = var("AWS_SHARED_CREDENTIALS_FILE")
            .map(Into::into)
            .or_else(|| {
                let home = var("HOME").or_else(|| var("USERPROFILE"))?;
                Some(std::path::Path::new(&home).join(".aws").join("credentials"))
            });
        let profile = var("AWS_PROFILE").unwrap_or_else(|| "default".into());
        file.and_then(|f| std::fs::read_to_string(f).ok())
            .and_then(|t| parse_profile(&t, &profile))
    };
    let (k, s, t) = found.ok_or_else(|| {
        Error::PermissionDenied(
            "sin credenciales S3: define AWS_ACCESS_KEY_ID/AWS_SECRET_ACCESS_KEY \
             (y AWS_SESSION_TOKEN) o un perfil en ~/.aws/credentials"
                .into(),
        )
    })?;
    Ok(match t {
        Some(t) => Credentials::new_with_token(k, s, t),
        None => Credentials::new(k, s),
    })
}

struct Inner {
    client: Client,
    bucket: Bucket,
    creds: Credentials,
}

pub struct S3Fs(Arc<Inner>);

fn loopback(u: &Url) -> bool {
    u.host_str().is_some_and(|h| {
        h == "localhost"
            || h.trim_matches(['[', ']'])
                .parse::<std::net::IpAddr>()
                .is_ok_and(|a| a.is_loopback())
    })
}

impl S3Fs {
    pub fn new(o: &S3Options, creds: Credentials) -> Result<Self> {
        let endpoint = o
            .endpoint
            .clone()
            .unwrap_or_else(|| format!("https://s3.{}.amazonaws.com", o.region));
        let endpoint =
            Url::parse(&endpoint).map_err(|e| Error::Protocol(format!("endpoint: {e}")))?;
        // Las credenciales firman peticiones: solo viajan en claro a loopback (o con opt-in).
        if endpoint.scheme() == "http"
            && !loopback(&endpoint)
            && std::env::var("FRIKI_S3_INSECURE").as_deref() != Ok("1")
        {
            return Err(Error::Protocol(
                "endpoint S3 http:// no local; usa https o FRIKI_S3_INSECURE=1".into(),
            ));
        }
        let style = if o.path_style || o.endpoint.is_some() || o.bucket.contains('.') {
            UrlStyle::Path
        } else {
            UrlStyle::VirtualHost
        };
        let bucket = Bucket::new(endpoint, style, o.bucket.clone(), o.region.clone())
            .map_err(|e| Error::Protocol(format!("bucket: {e}")))?;
        Ok(Self(Arc::new(Inner {
            // Sin redirecciones: un 301 de región erróneo se informa, no se sigue.
            client: client(false)?,
            bucket,
            creds,
        })))
    }
}

/// Clave de objeto a partir de una ruta (sin `/` inicial ni `.`).
fn key(path: &str) -> &str {
    let p = path.trim_start_matches('/');
    if p == "." { "" } else { p }
}

/// Error de S3 con su `<Code>` si el cuerpo lo trae.
fn s3_err(status: reqwest::StatusCode, body: &str, what: &str) -> Error {
    let code = body
        .split_once("<Code>")
        .and_then(|(_, r)| r.split_once("</Code>"))
        .map(|(c, _)| c);
    match (status, code) {
        (_, Some("NoSuchKey" | "NoSuchBucket")) => Error::NotFound(what.to_string()),
        (_, Some("AccessDenied" | "InvalidAccessKeyId" | "SignatureDoesNotMatch")) => {
            Error::PermissionDenied(format!("{what}: {}", code.unwrap_or("")))
        }
        (s, Some(c)) => Error::Protocol(format!("{what}: HTTP {s} {c}")),
        (s, None) => map_status(s, what),
    }
}

/// `YYYY-MM-DDTHH:MM:SS[.fff]Z` a segundos Unix.
fn parse_iso(s: &str) -> Option<i64> {
    let n = |r: std::ops::Range<usize>| s.get(r)?.parse::<i64>().ok();
    let (y, m, d) = (n(0..4)?, n(5..7)?, n(8..10)?);
    // Días desde la época (algoritmo de Hinnant).
    let y = y - i64::from(m <= 2);
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(days * 86_400 + n(11..13)? * 3600 + n(14..16)? * 60 + n(17..19)?)
}

/// Codifica una clave para `x-amz-copy-source` (todo salvo `A-Za-z0-9-_.~/`).
fn pct(s: &str) -> String {
    s.bytes().fold(String::new(), |mut o, b| {
        if b.is_ascii_alphanumeric() || b"-_.~/".contains(&b) {
            o.push(b as char);
        } else {
            o.push_str(&format!("%{b:02X}"));
        }
        o
    })
}

impl Inner {
    /// Envía y exige 2xx; devuelve la respuesta.
    async fn send(&self, req: RequestBuilder, what: &str) -> Result<Response> {
        let r = req.send().await.map_err(map_err)?;
        if r.status().is_success() {
            return Ok(r);
        }
        let st = r.status();
        let body = r.text().await.unwrap_or_default();
        Err(s3_err(st, &body, what))
    }

    async fn text(&self, req: RequestBuilder, what: &str) -> Result<String> {
        self.send(req, what).await?.text().await.map_err(map_err)
    }

    /// Un 200 de S3 puede traer `<Error>` en el cuerpo (CopyObject, Complete...).
    fn check_body(body: &str, what: &str) -> Result<()> {
        if body.contains("<Error>") {
            return Err(s3_err(reqwest::StatusCode::OK, body, what));
        }
        Ok(())
    }

    async fn list_page(
        &self,
        prefix: &str,
        delim: bool,
        max: Option<usize>,
        token: Option<&str>,
    ) -> Result<rusty_s3::actions::ListObjectsV2Response> {
        let mut a = ListObjectsV2::new(&self.bucket, Some(&self.creds));
        a.with_prefix(prefix);
        if delim {
            a.with_delimiter("/");
        }
        if let Some(m) = max {
            a.with_max_keys(m);
        }
        if let Some(t) = token {
            a.with_continuation_token(t);
        }
        let body = self.text(self.client.get(a.sign(TTL)), prefix).await?;
        ListObjectsV2::parse_response(&body).map_err(|e| Error::Protocol(format!("lista S3: {e}")))
    }

    async fn put_part(
        &self,
        key: &str,
        up: &mut (String, Vec<String>),
        data: Vec<u8>,
    ) -> Result<()> {
        let n = u16::try_from(up.1.len() + 1)
            .map_err(|_| Error::Protocol("objeto con demasiadas partes".into()))?;
        let a = UploadPart::new(&self.bucket, Some(&self.creds), key, n, &up.0);
        let r = self
            .send(self.client.put(a.sign(TTL)).body(data), key)
            .await?;
        let etag = r
            .headers()
            .get(reqwest::header::ETAG)
            .and_then(|v| v.to_str().ok())
            .ok_or_else(|| Error::Protocol("parte S3 sin ETag".into()))?;
        up.1.push(etag.to_string());
        Ok(())
    }

    async fn upload_inner(
        &self,
        key: &str,
        rx: &mut mpsc::Receiver<Vec<u8>>,
        up: &mut Option<(String, Vec<String>)>,
    ) -> Result<()> {
        let mut buf: Vec<u8> = Vec::new();
        loop {
            let done = match rx.recv().await {
                Some(c) => {
                    buf.extend_from_slice(&c);
                    false
                }
                None => true,
            };
            if done {
                break;
            }
            if buf.len() >= PART {
                if up.is_none() {
                    let a = CreateMultipartUpload::new(&self.bucket, Some(&self.creds), key);
                    let body = self.text(self.client.post(a.sign(TTL)), key).await?;
                    let r = CreateMultipartUpload::parse_response(&body)
                        .map_err(|e| Error::Protocol(format!("multipart S3: {e}")))?;
                    *up = Some((r.upload_id().to_string(), Vec::new()));
                }
                let part = std::mem::take(&mut buf);
                self.put_part(key, up.as_mut().expect("creado arriba"), part)
                    .await?;
            }
        }
        let Some(u) = up.as_mut() else {
            // Objeto pequeño: un solo PutObject.
            let a = PutObject::new(&self.bucket, Some(&self.creds), key);
            self.send(self.client.put(a.sign(TTL)).body(buf), key)
                .await?;
            return Ok(());
        };
        if !buf.is_empty() {
            self.put_part(key, u, buf).await?;
        }
        let a = CompleteMultipartUpload::new(
            &self.bucket,
            Some(&self.creds),
            key,
            &u.0,
            u.1.iter().map(String::as_str),
        );
        let url = a.sign(TTL);
        let body = self.text(self.client.post(url).body(a.body()), key).await?;
        CompleteMultipartUploadResponse::parse(&body)
            .map_err(|e| Error::Protocol(format!("completar multipart S3: {e}")))?;
        Ok(())
    }

    async fn upload(&self, key: String, mut rx: mpsc::Receiver<Vec<u8>>) -> Result<()> {
        let mut up = None;
        let r = self.upload_inner(&key, &mut rx, &mut up).await;
        if r.is_err()
            && let Some((id, _)) = &up
        {
            let a = AbortMultipartUpload::new(&self.bucket, Some(&self.creds), &key, id);
            let _ = self.client.delete(a.sign(TTL)).send().await;
        }
        r
    }
}

/// Escritor que reenvía a una tarea de subida; `shutdown` espera a que termine (commit).
struct S3Writer {
    tx: Option<PollSender<Vec<u8>>>,
    task: Option<tokio::task::JoinHandle<Result<()>>>,
}

impl AsyncWrite for S3Writer {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let broken = || io::Error::from(io::ErrorKind::BrokenPipe);
        let tx = self.tx.as_mut().ok_or_else(broken)?;
        match tx.poll_reserve(cx) {
            Poll::Ready(Ok(())) => {
                tx.send_item(buf.to_vec()).map_err(|_| broken())?;
                Poll::Ready(Ok(buf.len()))
            }
            Poll::Ready(Err(_)) => Poll::Ready(Err(broken())),
            Poll::Pending => Poll::Pending,
        }
    }

    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.tx = None; // cierra el canal: la tarea sube lo pendiente y completa
        let Some(t) = self.task.as_mut() else {
            return Poll::Ready(Ok(()));
        };
        let r = std::task::ready!(Pin::new(t).poll(cx));
        self.task = None;
        Poll::Ready(match r {
            Ok(Ok(())) => Ok(()),
            Ok(Err(e)) => Err(io::Error::other(e.to_string())),
            Err(e) => Err(io::Error::other(e.to_string())),
        })
    }
}

#[async_trait::async_trait]
impl RemoteFs for S3Fs {
    async fn list(&self, path: &str) -> Result<Vec<Entry>> {
        let mut prefix = key(path).to_string();
        if !prefix.is_empty() && !prefix.ends_with('/') {
            prefix.push('/');
        }
        let (mut out, mut token) = (Vec::new(), None::<String>);
        loop {
            let p = self
                .0
                .list_page(&prefix, true, None, token.as_deref())
                .await?;
            for c in p.contents {
                let name = c.key.strip_prefix(&prefix).unwrap_or(&c.key);
                if name.is_empty() || !safe_name(name) {
                    continue; // marcador del propio prefijo o nombre no seguro
                }
                out.push(Entry {
                    name: name.to_string(),
                    path: join(path, name),
                    kind: Kind::File,
                    size: c.size.unwrap_or(0),
                    mtime: c.last_modified.as_deref().and_then(parse_iso),
                    mode: None,
                });
            }
            for c in p.common_prefixes {
                let name = c
                    .prefix
                    .strip_prefix(&prefix)
                    .unwrap_or(&c.prefix)
                    .trim_end_matches('/');
                if safe_name(name) {
                    out.push(Entry {
                        name: name.to_string(),
                        path: join(path, name),
                        kind: Kind::Dir,
                        size: 0,
                        mtime: None,
                        mode: None,
                    });
                }
            }
            match p.next_continuation_token {
                Some(t) => token = Some(t),
                None => return Ok(out),
            }
        }
    }

    async fn stat(&self, path: &str) -> Result<Option<Entry>> {
        let k = key(path).trim_end_matches('/');
        let name = k.rsplit('/').next().unwrap_or("").to_string();
        let dir = |_: ()| Entry {
            name: name.clone(),
            path: path.to_string(),
            kind: Kind::Dir,
            size: 0,
            mtime: None,
            mode: None,
        };
        if k.is_empty() {
            return Ok(Some(dir(())));
        }
        // El .friki-part no se reanuda (no se puede añadir a un objeto): se ve como inexistente.
        if k.ends_with(PART_SUFFIX) {
            return Ok(None);
        }
        if !path.ends_with('/') {
            let a = HeadObject::new(&self.0.bucket, Some(&self.0.creds), k);
            let r = self
                .0
                .client
                .head(a.sign(TTL))
                .send()
                .await
                .map_err(map_err)?;
            if r.status().is_success() {
                return Ok(Some(Entry {
                    name,
                    path: path.to_string(),
                    kind: Kind::File,
                    size: content_len(&r),
                    mtime: last_modified(&r),
                    mode: None,
                }));
            }
            if r.status() != reqwest::StatusCode::NOT_FOUND {
                return Err(map_status(r.status(), path));
            }
        }
        // ¿Hay algo bajo el prefijo `k/`?
        let p = self
            .0
            .list_page(&format!("{k}/"), true, Some(1), None)
            .await?;
        Ok((!p.contents.is_empty() || !p.common_prefixes.is_empty()).then(|| dir(())))
    }

    async fn mkdir(&self, _: &str) -> Result<()> {
        Ok(()) // los prefijos existen al escribir un objeto
    }

    async fn remove_file(&self, path: &str) -> Result<()> {
        let a = DeleteObject::new(&self.0.bucket, Some(&self.0.creds), key(path));
        self.0.send(self.0.client.delete(a.sign(TTL)), path).await?;
        Ok(())
    }

    async fn remove_dir(&self, _: &str) -> Result<()> {
        Ok(())
    }

    async fn rename(&self, from: &str, to: &str) -> Result<()> {
        // ponytail: CopyObject único (<= 5 GiB); mayores exigirían UploadPartCopy.
        let head = HeadObject::new(&self.0.bucket, Some(&self.0.creds), key(from));
        let r = self
            .0
            .client
            .head(head.sign(TTL))
            .send()
            .await
            .map_err(map_err)?;
        if !r.status().is_success() {
            return Err(map_status(r.status(), from));
        }
        if content_len(&r) > COPY_MAX {
            return Err(unsupported("renombrar objetos de más de 5 GiB"));
        }
        let mut a = PutObject::new(&self.0.bucket, Some(&self.0.creds), key(to));
        let src = format!("/{}/{}", self.0.bucket.name(), pct(key(from)));
        a.headers_mut().insert("x-amz-copy-source", src.clone());
        let req = self
            .0
            .client
            .request(Method::PUT, a.sign(TTL))
            .header("x-amz-copy-source", src);
        let body = self.0.text(req, to).await?;
        Inner::check_body(&body, to)?;
        self.remove_file(from).await
    }

    async fn open_read(&self, path: &str, offset: u64) -> Result<Reader> {
        let a = GetObject::new(&self.0.bucket, Some(&self.0.creds), key(path));
        let mut req = self.0.client.get(a.sign(TTL));
        if offset > 0 {
            // El Range no se firma (URL prefirmada): basta con enviarlo.
            req = req.header(reqwest::header::RANGE, format!("bytes={offset}-"));
        }
        let r = self.0.send(req, path).await?;
        if offset > 0 && r.status() != reqwest::StatusCode::PARTIAL_CONTENT {
            return Err(Error::Protocol(
                "el servidor no admite reanudar (Range)".into(),
            ));
        }
        Ok(body_reader(r))
    }

    async fn open_write(&self, path: &str, offset: u64) -> Result<Writer> {
        if offset > 0 {
            return Err(unsupported("reanudar una subida"));
        }
        let (tx, rx) = mpsc::channel(4);
        let inner = self.0.clone();
        let k = key(path).to_string();
        let task = tokio::spawn(async move { inner.upload(k, rx).await });
        Ok(Box::new(S3Writer {
            tx: Some(PollSender::new(tx)),
            task: Some(task),
        }))
    }

    /// La fecha del objeto la fija el servidor.
    async fn set_mtime(&self, _: &str, _: i64) -> Result<()> {
        Ok(())
    }

    /// Multipart: el objeto aparece completo solo al cerrar; no hace falta `.friki-part`.
    fn atomic_writes(&self) -> bool {
        true
    }

    fn label(&self) -> String {
        format!("s3://{}", self.0.bucket.name())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn perfil_aws() {
        let t = "[default]\naws_access_key_id = AK\naws_secret_access_key=SK\n\n[dev]\naws_access_key_id = A2\naws_secret_access_key = S2\naws_session_token = T2\n";
        assert_eq!(
            parse_profile(t, "default"),
            Some(("AK".into(), "SK".into(), None))
        );
        assert_eq!(
            parse_profile(t, "dev"),
            Some(("A2".into(), "S2".into(), Some("T2".into())))
        );
        assert_eq!(parse_profile(t, "otro"), None);
    }

    #[test]
    fn copy_200_con_error_en_el_cuerpo_falla() {
        let err = "<Error><Code>InternalError</Code></Error>";
        assert!(Inner::check_body(err, "k").is_err());
        assert!(Inner::check_body("<CopyObjectResult/>", "k").is_ok());
    }

    #[test]
    fn fechas_y_claves() {
        assert_eq!(parse_iso("2026-01-01T00:00:00.000Z"), Some(1_767_225_600));
        assert_eq!(parse_iso("1970-01-02T00:00:01Z"), Some(86_401));
        assert_eq!(pct("a b/ñ+.txt"), "a%20b/%C3%B1%2B.txt");
        assert_eq!((key("/a/b"), key("."), key("")), ("a/b", "", ""));
    }

    #[test]
    fn http_no_local_se_rechaza() {
        let mut o = S3Options::from_env("b");
        o.endpoint = Some("http://example.com".into());
        assert!(S3Fs::new(&o, Credentials::new("a", "b")).is_err());
        o.endpoint = Some("http://127.0.0.1:9000".into());
        assert!(S3Fs::new(&o, Credentials::new("a", "b")).is_ok());
    }

    /// Contra MinIO real (scripts/test-minio.sh): `FRIKI_TEST_S3_ENDPOINT` y credenciales en el entorno.
    #[tokio::test]
    #[ignore = "necesita MinIO (scripts/test-minio.sh)"]
    async fn minio_ciclo_completo() {
        use rusty_s3::actions::CreateBucket;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        // MinIO no entra en FRIKI_TEST_REQUIRED (la CI no lo levanta): sin endpoint, se salta.
        let Ok(ep) = std::env::var("FRIKI_TEST_S3_ENDPOINT") else {
            eprintln!("aviso: sin FRIKI_TEST_S3_ENDPOINT, prueba MinIO saltada");
            return;
        };
        let name = std::env::var("FRIKI_TEST_S3_BUCKET").unwrap_or("friki-test".into());
        let mut o = S3Options::from_env(&name);
        let creds = credentials_from_env().unwrap();
        // Bucket propio: si ya existe (reejecución) no es error.
        let b = Bucket::new(
            Url::parse(&ep).unwrap(),
            UrlStyle::Path,
            name,
            o.region.clone(),
        )
        .unwrap();
        let c = CreateBucket::new(&b, &creds);
        let r = reqwest::Client::new()
            .put(c.sign(TTL))
            .body(c.body())
            .send()
            .await
            .unwrap();
        let st = r.status();
        let txt = r.text().await.unwrap_or_default();
        assert!(
            st.is_success() || txt.contains("BucketAlreadyOwnedByYou"),
            "crear bucket: {st} {txt}"
        );
        o.endpoint = Some(ep);
        let fs = S3Fs::new(&o, creds).unwrap();
        // 40 MiB: fuerza multipart (3 partes de 16 MiB).
        let data: Vec<u8> = (0..40u32 << 20).map(|i| (i % 251) as u8).collect();
        let part = format!("t/grande.bin{PART_SUFFIX}");
        let mut w = fs.open_write(&part, 0).await.unwrap();
        w.write_all(&data).await.unwrap();
        w.shutdown().await.unwrap();
        fs.rename(&part, "t/grande.bin").await.unwrap();
        let e = fs.stat("t/grande.bin").await.unwrap().unwrap();
        assert_eq!(e.size, data.len() as u64);
        assert!(fs.stat(&part).await.unwrap().is_none());
        // Pequeño (PutObject) y listado con prefijo.
        let mut w = fs.open_write("t/sub/p.txt", 0).await.unwrap();
        w.write_all(b"hola").await.unwrap();
        w.shutdown().await.unwrap();
        let l = fs.list("t").await.unwrap();
        let mut n: Vec<_> = l.iter().map(|e| (e.name.as_str(), e.kind)).collect();
        n.sort_by_key(|x| x.0);
        assert_eq!(n, [("grande.bin", Kind::File), ("sub", Kind::Dir)]);
        assert_eq!(fs.stat("t/sub").await.unwrap().unwrap().kind, Kind::Dir);
        assert!(fs.stat("t/nada").await.unwrap().is_none());
        // Lectura con Range.
        let mut v = Vec::new();
        fs.open_read("t/grande.bin", 39 << 20)
            .await
            .unwrap()
            .read_to_end(&mut v)
            .await
            .unwrap();
        assert_eq!(v, &data[39 << 20..]);
        fs.remove_file("t/grande.bin").await.unwrap();
        fs.remove_file("t/sub/p.txt").await.unwrap();
        assert!(fs.list("t").await.unwrap().is_empty());
    }
}
