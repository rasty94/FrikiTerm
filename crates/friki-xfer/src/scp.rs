//! SCP (protocolo `scp -t` / `scp -f`) sobre `Connection::open_exec_stream`.
//! Todo lo que llega del remoto se valida: nombres (`crate::safe_name`), tamaños y modos.

use std::future::Future;
use std::path::Path;
use std::pin::Pin;
use std::time::{Duration, UNIX_EPOCH};

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::{Error, Result, local_target, safe_name};

/// Tope de un tamaño anunciado (1 TiB): por encima se trata como mensaje corrupto.
const MAX_FILE_SIZE: u64 = 1 << 40;
/// Longitud máxima de una línea de control.
const MAX_LINE: usize = 4096;
/// Profundidad máxima de directorios anidados.
const MAX_DEPTH: usize = 64;

fn proto(msg: impl Into<String>) -> Error {
    Error::Protocol(msg.into())
}

/// Comillas simples seguras para el shell remoto.
fn quote(path: &str) -> String {
    format!("'{}'", path.replace('\'', "'\\''"))
}

/// Sube un fichero o, con `recursive`, un directorio.
pub async fn upload(
    conn: &friki_ssh::Connection,
    local: &Path,
    remote: &str,
    recursive: bool,
) -> Result<u64> {
    let flags = if recursive { "-r -p -t" } else { "-p -t" };
    let mut s = conn
        .open_exec_stream(&format!("scp {flags} -- {}", quote(remote)))
        .await?;
    upload_stream(&mut s, local, recursive).await
}

/// Descarga en `local_dir`. Nunca escribe fuera de `local_dir`.
pub async fn download(
    conn: &friki_ssh::Connection,
    remote: &str,
    local_dir: &Path,
    recursive: bool,
) -> Result<u64> {
    let flags = if recursive { "-r -p -f" } else { "-p -f" };
    let mut s = conn
        .open_exec_stream(&format!("scp {flags} -- {}", quote(remote)))
        .await?;
    download_stream(&mut s, local_dir, recursive).await
}

// ---------------------------------------------------------------- subida (lado fuente)

async fn read_ack<S: AsyncRead + Unpin>(s: &mut S) -> Result<()> {
    match s.read_u8().await? {
        0 => Ok(()),
        1 | 2 => {
            let msg = read_line(s).await.unwrap_or_default();
            Err(proto(format!("scp remoto: {msg}")))
        }
        b => Err(proto(format!("respuesta scp inesperada: {b:#x}"))),
    }
}

async fn read_line<S: AsyncRead + Unpin>(s: &mut S) -> Result<String> {
    let mut buf = Vec::new();
    loop {
        let b = s.read_u8().await?;
        if b == b'\n' {
            return Ok(String::from_utf8_lossy(&buf).into_owned());
        }
        if buf.len() >= MAX_LINE {
            return Err(proto("línea de control demasiado larga"));
        }
        buf.push(b);
    }
}

/// Sube sobre un flujo ya abierto hacia `scp -t`.
pub async fn upload_stream<S>(s: &mut S, local: &Path, recursive: bool) -> Result<u64>
where
    S: AsyncRead + AsyncWrite + Unpin + Send,
{
    read_ack(s).await?;
    let n = send_path(s, local, recursive, 0).await?;
    s.shutdown().await?;
    Ok(n)
}

fn send_path<'a, S>(
    s: &'a mut S,
    path: &'a Path,
    recursive: bool,
    depth: usize,
) -> Pin<Box<dyn Future<Output = Result<u64>> + Send + 'a>>
where
    S: AsyncRead + AsyncWrite + Unpin + Send,
{
    Box::pin(async move {
        if depth > MAX_DEPTH {
            return Err(proto("directorio demasiado profundo"));
        }
        let meta = tokio::fs::symlink_metadata(path).await?;
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .filter(|n| safe_name(n) && !n.contains('\n'))
            .ok_or_else(|| proto(format!("nombre no enviable: {}", path.display())))?
            .to_string();
        let (mtime, mode) = mtime_mode(&meta);
        s.write_all(format!("T{mtime} 0 {mtime} 0\n").as_bytes())
            .await?;
        read_ack(s).await?;
        if meta.is_dir() {
            if !recursive {
                return Err(proto("es un directorio: usa recursive"));
            }
            s.write_all(format!("D{mode:04o} 0 {name}\n").as_bytes())
                .await?;
            read_ack(s).await?;
            let mut total = 0;
            let mut rd = tokio::fs::read_dir(path).await?;
            while let Some(e) = rd.next_entry().await? {
                // Enlaces y ficheros especiales se omiten (no se siguen enlaces).
                let t = e.file_type().await?;
                if t.is_file() || t.is_dir() {
                    total += send_path(s, &e.path(), recursive, depth + 1).await?;
                }
            }
            s.write_all(b"E\n").await?;
            read_ack(s).await?;
            Ok(total)
        } else if meta.is_file() {
            let size = meta.len();
            s.write_all(format!("C{mode:04o} {size} {name}\n").as_bytes())
                .await?;
            read_ack(s).await?;
            let mut f = tokio::fs::File::open(path).await?;
            let mut left = size;
            let mut buf = vec![0u8; 32 * 1024];
            while left > 0 {
                let want = buf.len().min(usize::try_from(left).unwrap_or(buf.len()));
                let n = f.read(&mut buf[..want]).await?;
                if n == 0 {
                    return Err(proto("el fichero local cambió durante la subida"));
                }
                s.write_all(&buf[..n]).await?;
                left -= n as u64;
            }
            s.write_all(&[0]).await?;
            read_ack(s).await?;
            Ok(size)
        } else {
            Err(proto(format!(
                "no es fichero ni directorio: {}",
                path.display()
            )))
        }
    })
}

fn mtime_mode(m: &std::fs::Metadata) -> (u64, u32) {
    let t = m
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_secs());
    #[cfg(unix)]
    let mode = {
        use std::os::unix::fs::PermissionsExt;
        m.permissions().mode() & 0o777
    };
    #[cfg(not(unix))]
    let mode = if m.is_dir() { 0o755 } else { 0o644 };
    (t, mode)
}

// ---------------------------------------------------------------- descarga (lado sumidero)

/// Parsea `<modo-octal> <tamaño> <nombre>` (lo que sigue a `C`/`D`).
fn parse_header(line: &str) -> Result<(u32, u64, &str)> {
    let mut it = line.splitn(3, ' ');
    let (m, sz, name) = match (it.next(), it.next(), it.next()) {
        (Some(m), Some(s), Some(n)) => (m, s, n),
        _ => return Err(proto(format!("cabecera scp mal formada: {line:?}"))),
    };
    let mode = u32::from_str_radix(m, 8)
        .ok()
        .filter(|v| m.len() <= 5 && *v <= 0o7777)
        .ok_or_else(|| proto(format!("modo inválido: {m:?}")))?;
    let size: u64 = sz
        .parse()
        .map_err(|_| proto(format!("tamaño inválido: {sz:?}")))?;
    if !safe_name(name) {
        return Err(proto(format!("nombre rechazado: {name:?}")));
    }
    Ok((mode, size, name))
}

/// Descarga desde un flujo ya abierto hacia `scp -f`.
pub async fn download_stream<S>(s: &mut S, local_dir: &Path, recursive: bool) -> Result<u64>
where
    S: AsyncRead + AsyncWrite + Unpin + Send,
{
    let mut dirs = vec![local_dir.to_path_buf()];
    let mut total = 0u64;
    let mut mtime: Option<u64> = None;
    s.write_all(&[0]).await?;
    loop {
        let tag = match s.read_u8().await {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(e) => return Err(e.into()),
        };
        match tag {
            b'T' => {
                let line = read_line(s).await?;
                let t = line.split(' ').next().and_then(|v| v.parse::<u64>().ok());
                mtime = Some(t.ok_or_else(|| proto(format!("T mal formado: {line:?}")))?);
                s.write_all(&[0]).await?;
            }
            b'C' => {
                let line = read_line(s).await?;
                let (mode, size, name) = parse_header(&line)?;
                if size > MAX_FILE_SIZE {
                    return Err(proto(format!("tamaño absurdo: {size}")));
                }
                // dirs nunca está vacío: siempre contiene `local_dir`.
                let base = dirs
                    .last()
                    .ok_or_else(|| proto("pila de directorios vacía"))?;
                let dest = local_target(base, name).ok_or_else(|| proto("nombre rechazado"))?;
                refuse_symlink(&dest).await?;
                s.write_all(&[0]).await?;
                let mut f = tokio::fs::File::create(&dest).await?;
                let mut left = size;
                let mut buf = vec![0u8; 32 * 1024];
                while left > 0 {
                    let want = buf.len().min(usize::try_from(left).unwrap_or(buf.len()));
                    s.read_exact(&mut buf[..want]).await?;
                    f.write_all(&buf[..want]).await?;
                    left -= want as u64;
                }
                f.flush().await?;
                let std_f = f.into_std().await;
                finish_file(&std_f, mode, mtime.take())?;
                read_ack(s).await?;
                s.write_all(&[0]).await?;
                total += size;
            }
            b'D' => {
                if !recursive {
                    return Err(proto("el remoto envía un directorio sin recursive"));
                }
                if dirs.len() > MAX_DEPTH {
                    return Err(proto("directorio demasiado profundo"));
                }
                let line = read_line(s).await?;
                let (_, _, name) = parse_header(&line)?;
                let base = dirs
                    .last()
                    .ok_or_else(|| proto("pila de directorios vacía"))?;
                let dest = local_target(base, name).ok_or_else(|| proto("nombre rechazado"))?;
                refuse_symlink(&dest).await?;
                match tokio::fs::create_dir(&dest).await {
                    Ok(()) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                        if !tokio::fs::metadata(&dest).await?.is_dir() {
                            return Err(proto(format!("existe y no es directorio: {name}")));
                        }
                    }
                    Err(e) => return Err(e.into()),
                }
                dirs.push(dest);
                mtime = None;
                s.write_all(&[0]).await?;
            }
            b'E' => {
                let _ = read_line(s).await?;
                if dirs.len() <= 1 {
                    return Err(proto("E sin directorio abierto"));
                }
                dirs.pop();
                s.write_all(&[0]).await?;
            }
            1 | 2 => {
                let msg = read_line(s).await.unwrap_or_default();
                return Err(proto(format!("scp remoto: {msg}")));
            }
            b => return Err(proto(format!("mensaje scp inesperado: {b:#x}"))),
        }
    }
    if dirs.len() > 1 {
        return Err(proto("transferencia cortada con directorios abiertos"));
    }
    Ok(total)
}

/// No se escribe a través de un enlace simbólico preexistente.
async fn refuse_symlink(p: &Path) -> Result<()> {
    // ponytail: comprobar-y-crear no es atómico; un atacante local que compita no entra en el modelo.
    match tokio::fs::symlink_metadata(p).await {
        Ok(m) if m.file_type().is_symlink() => Err(proto(format!(
            "el destino es un enlace simbólico: {}",
            p.display()
        ))),
        _ => Ok(()),
    }
}

fn finish_file(f: &std::fs::File, mode: u32, mtime: Option<u64>) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // Sin setuid/setgid/sticky: solo rwx.
        f.set_permissions(std::fs::Permissions::from_mode(mode & 0o777))?;
    }
    #[cfg(not(unix))]
    let _ = mode;
    if let Some(t) = mtime {
        f.set_modified(UNIX_EPOCH + Duration::from_secs(t))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::duplex;

    /// Servidor `scp -f` simulado: envía `script` tal cual y acepta cualquier respuesta.
    async fn run_download(script: Vec<u8>, dir: &Path, rec: bool) -> Result<u64> {
        let (mut c, mut srv) = duplex(1 << 16);
        let h = tokio::spawn(async move {
            let _ = srv.write_all(&script).await;
            let _ = srv.shutdown().await;
            // Drena lo que envíe el cliente (acks) hasta cerrar.
            let mut sink = Vec::new();
            let _ = srv.read_to_end(&mut sink).await;
        });
        let r = download_stream(&mut c, dir, rec).await;
        drop(c);
        let _ = h.await;
        r
    }

    fn file_msg(name: &str, data: &[u8]) -> Vec<u8> {
        let mut v = format!("C0644 {} {name}\n", data.len()).into_bytes();
        v.extend_from_slice(data);
        v.push(0);
        v
    }

    #[test]
    fn quoting() {
        assert_eq!(quote("a b"), "'a b'");
        assert_eq!(quote("it's"), "'it'\\''s'");
        assert_eq!(quote("$(x);`y`"), "'$(x);`y`'");
    }

    #[tokio::test]
    async fn download_file_and_dir() {
        let d = tempfile::tempdir().unwrap();
        let mut sc = b"T1700000000 0 1700000000 0\n".to_vec();
        sc.extend(file_msg("a.txt", b"hola"));
        sc.extend(b"D0755 0 sub\n");
        sc.extend(file_msg("b.txt", b"mundo"));
        sc.extend(b"E\n");
        let n = run_download(sc, d.path(), true).await.unwrap();
        assert_eq!(n, 9);
        assert_eq!(std::fs::read(d.path().join("a.txt")).unwrap(), b"hola");
        assert_eq!(std::fs::read(d.path().join("sub/b.txt")).unwrap(), b"mundo");
        let m = std::fs::metadata(d.path().join("a.txt"))
            .unwrap()
            .modified()
            .unwrap();
        assert_eq!(m, UNIX_EPOCH + Duration::from_secs(1_700_000_000));
    }

    #[tokio::test]
    async fn rejects_malicious_names() {
        for name in ["../x", "a/b", "..", ".", "a\\b"] {
            let d = tempfile::tempdir().unwrap();
            let parent = d.path().join("in");
            std::fs::create_dir(&parent).unwrap();
            let r = run_download(file_msg(name, b"x"), &parent, true).await;
            assert!(matches!(r, Err(Error::Protocol(_))), "{name}: {r:?}");
            assert!(!d.path().join("x").exists());
            assert_eq!(std::fs::read_dir(&parent).unwrap().count(), 0);
            let r = run_download(format!("D0755 0 {name}\n").into_bytes(), &parent, true).await;
            assert!(matches!(r, Err(Error::Protocol(_))), "{name}: {r:?}");
        }
    }

    #[tokio::test]
    async fn rejects_bad_sizes_modes_and_messages() {
        let d = tempfile::tempdir().unwrap();
        for bad in [
            "C0644 99999999999999999999 f\n",
            "C0644 1099511627777 f\n",
            "C0644 -1 f\n",
            "C0644 abc f\n",
            "C0999 1 f\n",
            "C17777 1 f\n",
            "C0644 1\n",
            "X0644 1 f\n",
            "\x01boom\n",
            "E\n",
        ] {
            let r = run_download(bad.as_bytes().to_vec(), d.path(), true).await;
            assert!(matches!(r, Err(Error::Protocol(_))), "{bad:?}: {r:?}");
        }
        // Directorio sin -r, y transferencia truncada.
        let r = run_download(b"D0755 0 d\n".to_vec(), d.path(), false).await;
        assert!(matches!(r, Err(Error::Protocol(_))));
        let r = run_download(b"D0755 0 d\n".to_vec(), d.path(), true).await;
        assert!(
            matches!(r, Err(Error::Protocol(_))),
            "directorio sin cerrar"
        );
        let r = run_download(b"C0644 10 f\nabc".to_vec(), d.path(), true).await;
        assert!(r.is_err());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn refuses_symlink_destination() {
        let d = tempfile::tempdir().unwrap();
        let outside = d.path().join("outside");
        std::fs::write(&outside, "intacto").unwrap();
        let dir = d.path().join("in");
        std::fs::create_dir(&dir).unwrap();
        std::os::unix::fs::symlink(&outside, dir.join("f")).unwrap();
        let r = run_download(file_msg("f", b"pwn"), &dir, true).await;
        assert!(matches!(r, Err(Error::Protocol(_))));
        assert_eq!(std::fs::read(&outside).unwrap(), b"intacto");
    }

    /// `scp -t` simulado: acks de todo y registra lo recibido.
    async fn run_upload(path: &Path, rec: bool) -> (Result<u64>, Vec<u8>) {
        let (mut c, mut srv) = duplex(1 << 16);
        let h = tokio::spawn(async move {
            let mut got = Vec::new();
            let mut buf = [0u8; 4096];
            let _ = srv.write_all(&[0]).await;
            loop {
                match srv.read(&mut buf).await {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        got.extend_from_slice(&buf[..n]);
                        // Un ack por cada escritura del cliente (cada mensaje es una escritura).
                        for _ in 0..buf[..n].iter().filter(|b| **b == b'\n').count().max(1) {
                            let _ = srv.write_all(&[0]).await;
                        }
                    }
                }
            }
            got
        });
        let r = upload_stream(&mut c, path, rec).await;
        drop(c);
        (r, h.await.unwrap())
    }

    #[tokio::test]
    async fn upload_file_sends_c_message() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("f.txt");
        std::fs::write(&p, "abc").unwrap();
        let (r, got) = run_upload(&p, false).await;
        assert_eq!(r.unwrap(), 3);
        let s = String::from_utf8_lossy(&got).into_owned();
        assert!(s.contains(" f.txt\nabc\0"), "{s:?}");
        let (r, _) = run_upload(d.path(), false).await;
        assert!(r.is_err(), "directorio sin recursive");
    }
}
