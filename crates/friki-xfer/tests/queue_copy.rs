use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering::SeqCst};
use std::time::{Duration, Instant};

use friki_xfer::local::LocalFs;
use friki_xfer::queue::{Direction, Job, Queue, QueueEvent, QueueOptions, Resume};
use friki_xfer::{Entry, Error, ErrorKind, Reader, RemoteFs, Result, Writer};
use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};

/// Envoltorio que falla las `fails` primeras llamadas a `open_write`.
struct Flaky {
    inner: LocalFs,
    fails: AtomicU32,
}

#[async_trait::async_trait]
impl RemoteFs for Flaky {
    async fn list(&self, p: &str) -> Result<Vec<Entry>> {
        self.inner.list(p).await
    }
    async fn stat(&self, p: &str) -> Result<Option<Entry>> {
        self.inner.stat(p).await
    }
    async fn mkdir(&self, p: &str) -> Result<()> {
        self.inner.mkdir(p).await
    }
    async fn remove_file(&self, p: &str) -> Result<()> {
        self.inner.remove_file(p).await
    }
    async fn remove_dir(&self, p: &str) -> Result<()> {
        self.inner.remove_dir(p).await
    }
    async fn rename(&self, a: &str, b: &str) -> Result<()> {
        self.inner.rename(a, b).await
    }
    async fn open_read(&self, p: &str, o: u64) -> Result<Reader> {
        self.inner.open_read(p, o).await
    }
    async fn open_write(&self, p: &str, o: u64) -> Result<Writer> {
        if self.fails.load(SeqCst) > 0 {
            self.fails.fetch_sub(1, SeqCst);
            return Err(Error::Io(std::io::Error::other("caída simulada")));
        }
        self.inner.open_write(p, o).await
    }
    async fn set_mtime(&self, p: &str, m: i64) -> Result<()> {
        self.inner.set_mtime(p, m).await
    }
    fn label(&self) -> String {
        "flaky".into()
    }
}

fn fs(p: &Path) -> Arc<LocalFs> {
    Arc::new(LocalFs::new(p))
}

fn job(src: &str, dst: &str, recursive: bool) -> Job {
    Job {
        direction: Direction::Upload,
        src: src.into(),
        dst: dst.into(),
        recursive,
        resume: Resume::Auto,
        preserve_mtime: true,
    }
}

fn queue(
    l: Arc<dyn RemoteFs>,
    r: Arc<dyn RemoteFs>,
    opts: QueueOptions,
) -> (Queue, UnboundedReceiver<QueueEvent>) {
    let (tx, rx) = unbounded_channel();
    (Queue::new(l, r, opts, tx), rx)
}

fn drain(rx: &mut UnboundedReceiver<QueueEvent>) -> Vec<QueueEvent> {
    std::iter::from_fn(|| rx.try_recv().ok()).collect()
}

fn data(n: usize) -> Vec<u8> {
    (0..n).map(|i| (i % 251) as u8).collect()
}

#[tokio::test]
async fn copia_recursiva_conserva_mtime() {
    let (a, b) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    std::fs::create_dir_all(a.path().join("d/sub")).unwrap();
    std::fs::write(a.path().join("d/x.txt"), b"hola").unwrap();
    std::fs::write(a.path().join("d/sub/y.bin"), data(100_000)).unwrap();
    let (l, r) = (fs(a.path()), fs(b.path()));
    l.set_mtime("d/x.txt", 1_000_000_000).await.unwrap();
    let (q, mut rx) = queue(l, r.clone(), QueueOptions::default());
    q.push(job("d", "copia", true));
    assert_eq!(q.wait().await, (1, 0));
    assert_eq!(
        std::fs::read(b.path().join("copia/x.txt")).unwrap(),
        b"hola"
    );
    assert_eq!(
        std::fs::read(b.path().join("copia/sub/y.bin")).unwrap(),
        data(100_000)
    );
    let e = r.stat("copia/x.txt").await.unwrap().unwrap();
    assert_eq!(e.mtime, Some(1_000_000_000));
    assert!(matches!(
        drain(&mut rx).last(),
        Some(QueueEvent::Finished { bytes: 100_004, .. })
    ));
}

#[tokio::test]
async fn reanuda_fichero_a_medias_y_da_por_hecho_el_completo() {
    let (a, b) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let all = data(200_000);
    std::fs::write(a.path().join("f"), &all).unwrap();
    std::fs::write(b.path().join("f.friki-part"), &all[..50_000]).unwrap();
    let (l, r) = (fs(a.path()), fs(b.path()));
    l.set_mtime("f", 1_000_000_000).await.unwrap();
    let (q, mut rx) = queue(l, r, QueueOptions::default());
    q.push(job("f", "f", false));
    assert_eq!(q.wait().await, (1, 0));
    assert_eq!(std::fs::read(b.path().join("f")).unwrap(), all);
    assert!(
        drain(&mut rx)
            .iter()
            .any(|e| matches!(e, QueueEvent::Finished { bytes: 150_000, .. }))
    );
    // Mismo tamaño y mtime: no se copia nada.
    q.push(job("f", "f", false));
    assert_eq!(q.wait().await, (2, 0));
    assert!(
        drain(&mut rx)
            .iter()
            .any(|e| matches!(e, QueueEvent::Finished { bytes: 0, .. }))
    );
}

#[tokio::test]
async fn reintenta_ante_errores_de_io() {
    let (a, b) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    std::fs::write(a.path().join("f"), data(1000)).unwrap();
    let flaky = |n| {
        Arc::new(Flaky {
            inner: LocalFs::new(b.path()),
            fails: AtomicU32::new(n),
        })
    };
    let opts = QueueOptions {
        retries: 3,
        ..Default::default()
    };
    let (q, mut rx) = queue(fs(a.path()), flaky(2), opts.clone());
    q.push(job("f", "f", false));
    assert_eq!(q.wait().await, (1, 0));
    let n = drain(&mut rx)
        .iter()
        .filter(|e| matches!(e, QueueEvent::Retrying { .. }))
        .count();
    assert_eq!(n, 2);
    // Más fallos que reintentos: el trabajo falla.
    let (q, mut rx) = queue(fs(a.path()), flaky(10), opts);
    q.push(job("f", "g", false));
    assert_eq!(q.wait().await, (0, 1));
    assert!(
        drain(&mut rx)
            .iter()
            .any(|e| matches!(e, QueueEvent::Failed { .. }))
    );
}

#[tokio::test]
async fn cancelar_detiene_el_trabajo() {
    let (a, b) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    std::fs::write(a.path().join("f"), data(1 << 20)).unwrap();
    let opts = QueueOptions {
        bandwidth: Some(256 * 1024),
        ..Default::default()
    };
    let (q, mut rx) = queue(fs(a.path()), fs(b.path()), opts);
    let t = Instant::now();
    let id = q.push(job("f", "f", false));
    tokio::time::sleep(Duration::from_millis(300)).await;
    q.cancel(id);
    assert_eq!(q.wait().await, (0, 0));
    assert!(t.elapsed() < Duration::from_secs(3));
    assert!(drain(&mut rx).contains(&QueueEvent::Cancelled { id }));
    assert!(!b.path().join("f").exists());
    assert!(!b.path().join("f.friki-part").exists());
}

#[tokio::test]
async fn pausa_y_reanudacion() {
    let (a, b) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    std::fs::write(a.path().join("f"), data(4096)).unwrap();
    let (q, mut rx) = queue(fs(a.path()), fs(b.path()), QueueOptions::default());
    q.pause(true);
    q.push(job("f", "f", false));
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(drain(&mut rx).is_empty());
    assert!(!b.path().join("f").exists());
    q.pause(false);
    assert_eq!(q.wait().await, (1, 0));
    assert_eq!(std::fs::read(b.path().join("f")).unwrap(), data(4096));
}

#[tokio::test]
async fn limite_de_ancho_de_banda() {
    let (a, b) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    std::fs::write(a.path().join("f"), data(1 << 20)).unwrap();
    let opts = QueueOptions {
        bandwidth: Some(512 * 1024),
        ..Default::default()
    };
    let (q, _rx) = queue(fs(a.path()), fs(b.path()), opts);
    let t = Instant::now();
    q.push(job("f", "f", false));
    assert_eq!(q.wait().await, (1, 0));
    assert!(
        t.elapsed() >= Duration::from_millis(1800),
        "{:?}",
        t.elapsed()
    );
}

#[tokio::test]
async fn fallo_no_destruye_el_destino_anterior() {
    let (a, b) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    std::fs::write(a.path().join("f"), data(1 << 20)).unwrap();
    std::fs::write(b.path().join("f"), b"anterior").unwrap();
    let opts = QueueOptions {
        bandwidth: Some(256 * 1024),
        ..Default::default()
    };
    let (q, _rx) = queue(fs(a.path()), fs(b.path()), opts);
    let id = q.push(job("f", "f", false));
    tokio::time::sleep(Duration::from_millis(300)).await;
    q.cancel(id);
    q.wait().await;
    assert_eq!(std::fs::read(b.path().join("f")).unwrap(), b"anterior");
    assert!(!b.path().join("f.friki-part").exists());
}

#[tokio::test]
async fn resume_no_reanuda_sobre_un_destino_ajeno() {
    let (a, b) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let all = data(100_000);
    std::fs::write(a.path().join("f"), &all).unwrap();
    // Destino más corto que NO es un parcial propio: se reescribe entero, no se continúa.
    std::fs::write(b.path().join("f"), b"otra cosa").unwrap();
    let (q, mut rx) = queue(fs(a.path()), fs(b.path()), QueueOptions::default());
    q.push(job("f", "f", false));
    assert_eq!(q.wait().await, (1, 0));
    assert_eq!(std::fs::read(b.path().join("f")).unwrap(), all);
    assert!(
        drain(&mut rx)
            .iter()
            .any(|e| matches!(e, QueueEvent::Finished { bytes: 100_000, .. }))
    );
}

#[cfg(unix)]
#[tokio::test]
async fn permisos_descargados_acotados() {
    use std::os::unix::fs::PermissionsExt;
    let (a, b) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    std::fs::write(a.path().join("f"), b"x").unwrap();
    std::fs::set_permissions(a.path().join("f"), std::fs::Permissions::from_mode(0o4777)).unwrap();
    std::fs::write(a.path().join("s"), b"x").unwrap();
    std::fs::set_permissions(a.path().join("s"), std::fs::Permissions::from_mode(0o600)).unwrap();
    let (q, _rx) = queue(fs(a.path()), fs(b.path()), QueueOptions::default());
    q.push(job("f", "f", false));
    q.push(job("s", "s", false));
    assert_eq!(q.wait().await, (2, 0));
    let m = |n: &str| {
        std::fs::metadata(b.path().join(n))
            .unwrap()
            .permissions()
            .mode()
            & 0o7777
    };
    assert_eq!(m("f"), 0o755);
    assert_eq!(m("s"), 0o600);
}

#[tokio::test]
async fn failed_lleva_el_tipo_de_error() {
    let (a, b) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let (q, mut rx) = queue(fs(a.path()), fs(b.path()), QueueOptions::default());
    q.push(job("no-existe", "f", false));
    assert_eq!(q.wait().await, (0, 1));
    assert!(drain(&mut rx).iter().any(|e| matches!(
        e,
        QueueEvent::Failed {
            kind: ErrorKind::NotFound,
            ..
        }
    )));
}
