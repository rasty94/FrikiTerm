//! `RemoteFs` sobre el sistema de ficheros local (pruebas y lado local de la cola).

use std::path::{Component, Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tokio::io::{AsyncSeekExt, SeekFrom};

use crate::{Entry, Error, Kind, Reader, RemoteFs, Result, Writer, join};

/// Raíz desde la que se resuelven las rutas; nada fuera de ella es accesible.
pub struct LocalFs {
    pub root: PathBuf,
}

fn map_err(e: std::io::Error, path: &str) -> Error {
    match e.kind() {
        std::io::ErrorKind::NotFound => Error::NotFound(path.to_string()),
        std::io::ErrorKind::PermissionDenied => Error::PermissionDenied(path.to_string()),
        _ => Error::Io(e),
    }
}

fn secs(t: std::io::Result<SystemTime>) -> Option<i64> {
    let t = t.ok()?;
    Some(match t.duration_since(UNIX_EPOCH) {
        Ok(d) => d.as_secs() as i64,
        Err(e) => -(e.duration().as_secs() as i64),
    })
}

fn entry(name: String, path: String, md: &std::fs::Metadata) -> Entry {
    let ft = md.file_type();
    let kind = if ft.is_symlink() {
        Kind::Symlink
    } else if ft.is_dir() {
        Kind::Dir
    } else if ft.is_file() {
        Kind::File
    } else {
        Kind::Other
    };
    #[cfg(unix)]
    let mode = {
        use std::os::unix::fs::PermissionsExt;
        Some(md.permissions().mode() & 0o7777)
    };
    #[cfg(not(unix))]
    let mode = None;
    Entry {
        name,
        path,
        kind,
        size: if kind == Kind::File { md.len() } else { 0 },
        mtime: secs(md.modified()),
        mode,
    }
}

impl LocalFs {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// Resuelve `path` (relativa a `root`, separada por `/`). Rechaza `..`, rutas absolutas y
    /// enlaces simbólicos que apunten fuera de la raíz. Con `follow_last = false` el último
    /// componente no se sigue (así `stat`/`remove` actúan sobre el propio enlace).
    fn resolve(&self, path: &str, follow_last: bool) -> Result<PathBuf> {
        let denied = || Error::PermissionDenied(format!("ruta fuera de la raíz: {path}"));
        let root = self.root.canonicalize().map_err(|e| map_err(e, path))?;
        let mut target = root.clone();
        for comp in Path::new(path).components() {
            match comp {
                Component::Normal(c) => target.push(c),
                Component::CurDir => {}
                _ => return Err(denied()),
            }
        }
        // El ancestro existente más profundo (sin el último componente salvo `follow_last`)
        // se canonicaliza y debe seguir dentro de la raíz.
        let mut probe = if follow_last {
            target.clone()
        } else {
            target.parent().map(Path::to_path_buf).unwrap_or_default()
        };
        loop {
            match probe.canonicalize() {
                Ok(c) => {
                    if !c.starts_with(&root) {
                        return Err(denied());
                    }
                    break;
                }
                Err(_) => {
                    if !probe.pop() || !probe.starts_with(&root) {
                        return Err(denied());
                    }
                }
            }
        }
        Ok(target)
    }
}

#[async_trait::async_trait]
impl RemoteFs for LocalFs {
    async fn list(&self, path: &str) -> Result<Vec<Entry>> {
        let dir = self.resolve(path, true)?;
        let mut rd = tokio::fs::read_dir(&dir)
            .await
            .map_err(|e| map_err(e, path))?;
        let mut out = Vec::new();
        while let Some(e) = rd.next_entry().await? {
            let name = e.file_name().to_string_lossy().into_owned();
            let md = tokio::fs::symlink_metadata(e.path()).await?;
            let p = join(path, &name);
            out.push(entry(name, p, &md));
        }
        out.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(out)
    }

    async fn stat(&self, path: &str) -> Result<Option<Entry>> {
        let p = self.resolve(path, false)?;
        match tokio::fs::symlink_metadata(&p).await {
            Ok(md) => {
                let name = p
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                Ok(Some(entry(name, path.to_string(), &md)))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(map_err(e, path)),
        }
    }

    async fn mkdir(&self, path: &str) -> Result<()> {
        let p = self.resolve(path, false)?;
        tokio::fs::create_dir(&p)
            .await
            .map_err(|e| map_err(e, path))
    }

    async fn remove_file(&self, path: &str) -> Result<()> {
        let p = self.resolve(path, false)?;
        tokio::fs::remove_file(&p)
            .await
            .map_err(|e| map_err(e, path))
    }

    async fn remove_dir(&self, path: &str) -> Result<()> {
        let p = self.resolve(path, false)?;
        tokio::fs::remove_dir(&p)
            .await
            .map_err(|e| map_err(e, path))
    }

    async fn rename(&self, from: &str, to: &str) -> Result<()> {
        let (a, b) = (self.resolve(from, false)?, self.resolve(to, false)?);
        tokio::fs::rename(&a, &b)
            .await
            .map_err(|e| map_err(e, from))
    }

    async fn open_read(&self, path: &str, offset: u64) -> Result<Reader> {
        let p = self.resolve(path, true)?;
        let mut f = tokio::fs::File::open(&p)
            .await
            .map_err(|e| map_err(e, path))?;
        if offset > 0 {
            f.seek(SeekFrom::Start(offset)).await?;
        }
        Ok(Box::new(f))
    }

    async fn open_write(&self, path: &str, offset: u64) -> Result<Writer> {
        let p = self.resolve(path, true)?;
        let mut o = tokio::fs::OpenOptions::new();
        o.write(true).create(true);
        if offset == 0 {
            o.truncate(true);
        }
        #[cfg(unix)]
        o.mode(0o644);
        let mut f = o.open(&p).await.map_err(|e| map_err(e, path))?;
        if offset > 0 {
            f.seek(SeekFrom::Start(offset)).await?;
        }
        Ok(Box::new(f))
    }

    async fn set_mtime(&self, path: &str, mtime: i64) -> Result<()> {
        let p = self.resolve(path, true)?;
        let t = if mtime >= 0 {
            UNIX_EPOCH + Duration::from_secs(mtime as u64)
        } else {
            UNIX_EPOCH - Duration::from_secs(mtime.unsigned_abs())
        };
        let f = std::fs::File::options()
            .write(true)
            .open(&p)
            .map_err(|e| map_err(e, path))?;
        f.set_modified(t).map_err(|e| map_err(e, path))
    }

    fn label(&self) -> String {
        format!("file://{}", self.root.display())
    }
}
