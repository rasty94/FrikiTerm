//! SFTP sobre `friki_ssh::Connection::open_subsystem("sftp")` (cliente `russh-sftp`).

use std::io::SeekFrom;

use russh_sftp::client::SftpSession;
use russh_sftp::client::error::Error as SftpError;
use russh_sftp::protocol::{FileAttributes, FileType, OpenFlags, StatusCode};
use tokio::io::{AsyncRead, AsyncSeekExt, AsyncWrite};

use crate::{Entry, Error, Kind, Reader, RemoteFs, Result, Writer, join, safe_name};

/// Abre SFTP sobre una conexión SSH. `Connection` no expone usuario ni host, así que la
/// etiqueta es genérica; usa [`open_stream`] si necesitas `sftp://user@host`.
pub async fn open(conn: &friki_ssh::Connection) -> Result<Box<dyn RemoteFs>> {
    let stream = conn.open_subsystem("sftp").await?;
    open_stream(stream, "sftp://ssh").await
}

/// SFTP sobre cualquier flujo bidireccional ya conectado al servidor SFTP.
pub async fn open_stream<S>(stream: S, label: &str) -> Result<Box<dyn RemoteFs>>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let session = SftpSession::new(stream).await.map_err(map_err)?;
    Ok(Box::new(Sftp {
        session,
        label: label.to_string(),
    }))
}

struct Sftp {
    session: SftpSession,
    label: String,
}

fn map_err(e: SftpError) -> Error {
    match e {
        SftpError::Status(s) => match s.status_code {
            StatusCode::NoSuchFile => Error::NotFound(s.error_message),
            StatusCode::PermissionDenied => Error::PermissionDenied(s.error_message),
            _ => Error::Protocol(format!("{}: {}", s.status_code, s.error_message)),
        },
        e => Error::Protocol(e.to_string()),
    }
}

fn entry(name: String, path: String, a: &FileAttributes) -> Entry {
    Entry {
        name,
        path,
        kind: match a.file_type() {
            FileType::File => Kind::File,
            FileType::Dir => Kind::Dir,
            FileType::Symlink => Kind::Symlink,
            FileType::Other => Kind::Other,
        },
        size: a.size.unwrap_or(0),
        mtime: a.mtime.map(i64::from),
        mode: a.permissions.map(|m| m & 0o7777),
    }
}

#[async_trait::async_trait]
impl RemoteFs for Sftp {
    async fn list(&self, path: &str) -> Result<Vec<Entry>> {
        let dir = self.session.read_dir(path).await.map_err(map_err)?;
        // Un servidor hostil podría devolver nombres con `/` o `..`: se descartan.
        Ok(dir
            .filter_map(|e| {
                let name = e.file_name();
                safe_name(&name).then(|| entry(name.clone(), join(path, &name), &e.metadata()))
            })
            .collect())
    }

    async fn stat(&self, path: &str) -> Result<Option<Entry>> {
        match self.session.symlink_metadata(path).await {
            Ok(a) => {
                let name = path.rsplit('/').next().unwrap_or(path).to_string();
                Ok(Some(entry(name, path.to_string(), &a)))
            }
            Err(SftpError::Status(s)) if s.status_code == StatusCode::NoSuchFile => Ok(None),
            Err(e) => Err(map_err(e)),
        }
    }

    async fn mkdir(&self, path: &str) -> Result<()> {
        self.session.create_dir(path).await.map_err(map_err)
    }

    async fn remove_file(&self, path: &str) -> Result<()> {
        self.session.remove_file(path).await.map_err(map_err)
    }

    async fn remove_dir(&self, path: &str) -> Result<()> {
        self.session.remove_dir(path).await.map_err(map_err)
    }

    async fn rename(&self, from: &str, to: &str) -> Result<()> {
        self.session.rename(from, to).await.map_err(map_err)
    }

    async fn open_read(&self, path: &str, offset: u64) -> Result<Reader> {
        let mut f = self.session.open(path).await.map_err(map_err)?;
        if offset > 0 {
            f.seek(SeekFrom::Start(offset)).await?;
        }
        Ok(Box::new(f))
    }

    async fn open_write(&self, path: &str, offset: u64) -> Result<Writer> {
        if offset == 0 {
            let f = self.session.create(path).await.map_err(map_err)?;
            return Ok(Box::new(f));
        }
        let mut f = self
            .session
            .open_with_flags(path, OpenFlags::WRITE | OpenFlags::CREATE)
            .await
            .map_err(map_err)?;
        f.seek(SeekFrom::Start(offset)).await?;
        Ok(Box::new(f))
    }

    async fn set_mtime(&self, path: &str, mtime: i64) -> Result<()> {
        // SFTP v3 fija atime y mtime juntos.
        let t = u32::try_from(mtime)
            .map_err(|_| Error::Protocol(format!("mtime fuera de rango: {mtime}")))?;
        let attrs = FileAttributes {
            atime: Some(t),
            mtime: Some(t),
            ..FileAttributes::empty()
        };
        self.session
            .set_metadata(path, attrs)
            .await
            .map_err(map_err)
    }

    fn label(&self) -> String {
        self.label.clone()
    }
}
