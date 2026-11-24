use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering::SeqCst};
use std::task::{Context, Poll};

use friki_xfer::local::LocalFs;
use friki_xfer::queue::{Direction, Job, Queue, QueueOptions, Resume};
use friki_xfer::{Entry, Error, Reader, RemoteFs, Result, Writer};
use tokio::io::AsyncWrite;
use tokio::sync::mpsc::unbounded_channel;

/// `RemoteFs` que muere tras `limit` bytes escritos: desde entonces todo da error de E/S.
struct Dying {
    inner: LocalFs,
    limit: u64,
    written: Arc<AtomicU64>,
}

struct DyingWriter {
    inner: Writer,
    limit: u64,
    written: Arc<AtomicU64>,
}

impl AsyncWrite for DyingWriter {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        let done = self.written.load(SeqCst);
        if done >= self.limit {
            return Poll::Ready(Err(std::io::Error::other("conexión caída")));
        }
        let n = buf.len().min((self.limit - done) as usize);
        let r = Pin::new(&mut self.inner).poll_write(cx, &buf[..n]);
        if let Poll::Ready(Ok(w)) = &r {
            self.written.fetch_add(*w as u64, SeqCst);
        }
        r
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

impl Dying {
    fn alive(&self) -> Result<()> {
        if self.written.load(SeqCst) >= self.limit {
            return Err(Error::Io(std::io::Error::other("conexión caída")));
        }
        Ok(())
    }
}

#[async_trait::async_trait]
impl RemoteFs for Dying {
    async fn list(&self, p: &str) -> Result<Vec<Entry>> {
        self.alive()?;
        self.inner.list(p).await
    }
    async fn stat(&self, p: &str) -> Result<Option<Entry>> {
        self.alive()?;
        self.inner.stat(p).await
    }
    async fn mkdir(&self, p: &str) -> Result<()> {
        self.alive()?;
        self.inner.mkdir(p).await
    }
    async fn remove_file(&self, p: &str) -> Result<()> {
        self.alive()?;
        self.inner.remove_file(p).await
    }
    async fn remove_dir(&self, p: &str) -> Result<()> {
        self.alive()?;
        self.inner.remove_dir(p).await
    }
    async fn rename(&self, a: &str, b: &str) -> Result<()> {
        self.alive()?;
        self.inner.rename(a, b).await
    }
    async fn open_read(&self, p: &str, o: u64) -> Result<Reader> {
        self.alive()?;
        self.inner.open_read(p, o).await
    }
    async fn open_write(&self, p: &str, o: u64) -> Result<Writer> {
        self.alive()?;
        Ok(Box::new(DyingWriter {
            inner: self.inner.open_write(p, o).await?,
            limit: self.limit,
            written: self.written.clone(),
        }))
    }
    async fn set_mtime(&self, p: &str, m: i64) -> Result<()> {
        self.alive()?;
        self.inner.set_mtime(p, m).await
    }
    fn label(&self) -> String {
        "dying".into()
    }
}

#[tokio::test]
async fn reconecta_y_reanuda_tras_caida_a_mitad() {
    let (a, b) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let big: Vec<u8> = (0..200_000).map(|i| (i % 251) as u8).collect();
    std::fs::write(a.path().join("f.bin"), &big).unwrap();
    let dying = Arc::new(Dying {
        inner: LocalFs::new(b.path()),
        limit: 50_000,
        written: Arc::default(),
    });
    let reconnects = Arc::new(AtomicU32::new(0));
    let (rc, root) = (reconnects.clone(), b.path().to_path_buf());
    let (tx, mut rx) = unbounded_channel();
    let q = Queue::new(
        Arc::new(LocalFs::new(a.path())),
        dying,
        QueueOptions::default(),
        tx,
    )
    .with_reconnect(Arc::new(move || {
        rc.fetch_add(1, SeqCst);
        let root = root.clone();
        Box::pin(async move { Ok(Arc::new(LocalFs::new(&root)) as Arc<dyn RemoteFs>) })
    }));
    q.push(Job {
        direction: Direction::Upload,
        src: "f.bin".into(),
        dst: "f.bin".into(),
        recursive: false,
        resume: Resume::Auto,
        preserve_mtime: false,
    });
    let res = q.wait().await;
    let evs: Vec<_> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
    assert_eq!(res, (1, 0), "{evs:?}");
    assert_eq!(reconnects.load(SeqCst), 1);
    assert_eq!(std::fs::read(b.path().join("f.bin")).unwrap(), big);
}
