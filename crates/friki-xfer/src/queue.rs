//! Cola de transferencias: paralelismo, reintentos, reanudación, pausa, cancelación y
//! límite de ancho de banda (TODO.md §5).
//!
//! Reconexión opcional: con [`Queue::with_reconnect`], un error de red o de E/S pide un
//! `remote` nuevo antes de reintentar (y la copia se reanuda desde lo ya escrito).

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering::SeqCst};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{Semaphore, mpsc::UnboundedSender, watch};
use tokio::time::{Instant, sleep, sleep_until};

use crate::{Entry, Error, ErrorKind, Kind, RemoteFs, Result, join, safe_name};

/// Sufijo del fichero temporal en el que se escribe cada copia hasta que termina bien.
pub const PART_SUFFIX: &str = ".friki-part";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Upload,
    Download,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resume {
    /// Continuar un `<destino>.friki-part` propio más corto que el origen (y omitir el
    /// destino si ya es idéntico).
    Auto,
    Never,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Job {
    pub direction: Direction,
    pub src: String,
    pub dst: String,
    pub recursive: bool,
    pub resume: Resume,
    /// Conservar la fecha de modificación.
    pub preserve_mtime: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueueOptions {
    pub parallel: usize,
    pub retries: u32,
    /// Bytes por segundo en total; `None` = sin límite.
    pub bandwidth: Option<u64>,
}

impl Default for QueueOptions {
    fn default() -> Self {
        Self {
            parallel: 2,
            retries: 3,
            bandwidth: None,
        }
    }
}

pub type JobId = u64;

/// Fábrica de un `remote` nuevo tras perder la conexión.
pub type Reconnect =
    Arc<dyn Fn() -> Pin<Box<dyn Future<Output = Result<Arc<dyn RemoteFs>>> + Send>> + Send + Sync>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueueEvent {
    Started {
        id: JobId,
        file: String,
        total: Option<u64>,
    },
    Progress {
        id: JobId,
        file: String,
        done: u64,
    },
    Retrying {
        id: JobId,
        attempt: u32,
        error: String,
    },
    Finished {
        id: JobId,
        bytes: u64,
    },
    Failed {
        id: JobId,
        error: String,
        kind: ErrorKind,
    },
    Cancelled {
        id: JobId,
    },
}

const CHUNK: usize = 32 * 1024;
const PROGRESS_EVERY: Duration = Duration::from_millis(100);
const POLL: Duration = Duration::from_millis(25);

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Solo errores de E/S o de red merecen reintento.
fn retryable(e: &Error) -> bool {
    matches!(e, Error::Io(_) | Error::Ssh(_))
}

struct Inner {
    local: Arc<dyn RemoteFs>,
    remote: Mutex<Arc<dyn RemoteFs>>,
    reconnect: Mutex<Option<Reconnect>>,
    opts: QueueOptions,
    events: UnboundedSender<QueueEvent>,
    paused: AtomicBool,
    cancels: Mutex<HashMap<JobId, Arc<AtomicBool>>>,
    slots: Semaphore,
    /// Cubo de fichas compartido: instante a partir del cual cabe el siguiente bloque.
    bucket: Mutex<Instant>,
    pending: watch::Sender<usize>,
    ok: AtomicUsize,
    failed: AtomicUsize,
    next_id: AtomicU64,
}

pub struct Queue {
    inner: Arc<Inner>,
}

struct Ctx {
    id: JobId,
    cancel: Arc<AtomicBool>,
}

impl Inner {
    fn emit(&self, ev: QueueEvent) {
        let _ = self.events.send(ev);
    }

    /// Punto de control entre bloques: cancelación y pausa.
    async fn gate(&self, ctx: &Ctx) -> Result<()> {
        loop {
            if ctx.cancel.load(SeqCst) {
                return Err(Error::Cancelled);
            }
            if !self.paused.load(SeqCst) {
                return Ok(());
            }
            sleep(POLL).await;
        }
    }

    fn remote(&self) -> Arc<dyn RemoteFs> {
        lock(&self.remote).clone()
    }

    /// (origen, destino) con el `remote` vigente en este momento.
    fn ends(&self, dir: Direction) -> (Arc<dyn RemoteFs>, Arc<dyn RemoteFs>) {
        match dir {
            Direction::Upload => (self.local.clone(), self.remote()),
            Direction::Download => (self.remote(), self.local.clone()),
        }
    }

    /// Cambia el `remote` si nadie lo ha hecho ya (`used` es el que falló).
    async fn reconnect(&self, used: &Arc<dyn RemoteFs>) -> Result<()> {
        let Some(f) = lock(&self.reconnect).clone() else {
            return Ok(());
        };
        if Arc::ptr_eq(used, &self.remote()) {
            let new = f().await?;
            *lock(&self.remote) = new;
        }
        Ok(())
    }

    async fn throttle(&self, n: usize) {
        let Some(rate) = self.opts.bandwidth.filter(|r| *r > 0) else {
            return;
        };
        let until = {
            let mut next = lock(&self.bucket);
            let start = (*next).max(Instant::now());
            *next = start + Duration::from_secs_f64(n as f64 / rate as f64);
            *next
        };
        sleep_until(until).await;
    }

    /// Reintenta con espera exponencial (100 ms, 200 ms, ... máx. 5 s).
    async fn retry<T, F, Fut>(&self, ctx: &Ctx, mut f: F) -> Result<T>
    where
        F: FnMut() -> Fut,
        Fut: Future<Output = Result<T>>,
    {
        let mut attempt = 0u32;
        loop {
            self.gate(ctx).await?;
            let used = self.remote();
            match f().await {
                Err(e) if retryable(&e) && attempt < self.opts.retries => {
                    attempt += 1;
                    self.emit(QueueEvent::Retrying {
                        id: ctx.id,
                        attempt,
                        error: e.to_string(),
                    });
                    let ms = (100u64 << (attempt - 1).min(6)).min(5000);
                    sleep(Duration::from_millis(ms)).await;
                    // Si la reconexión falla, el siguiente intento fallará y gastará reintentos.
                    if let Err(e) = self.reconnect(&used).await {
                        self.emit(QueueEvent::Retrying {
                            id: ctx.id,
                            attempt,
                            error: format!("reconexión: {e}"),
                        });
                    }
                }
                r => return r,
            }
        }
    }

    /// Copia un fichero reanudando si procede. Devuelve los bytes copiados en este intento.
    async fn copy_file(
        &self,
        ctx: &Ctx,
        job: &Job,
        src: &str,
        dst: &str,
        s: &Entry,
    ) -> Result<u64> {
        let (sfs, dfs) = self.ends(job.direction);
        // Se escribe en `<dst>.friki-part` y se renombra al terminar: un fallo no destruye
        // el destino anterior.
        let part = format!("{dst}{PART_SUFFIX}");
        let mut offset = 0;
        if job.resume == Resume::Auto {
            if let Some(d) = dfs.stat(dst).await?.filter(|d| d.kind == Kind::File)
                && d.size == s.size
                && s.mtime.is_some()
                && d.mtime == s.mtime
            {
                return Ok(0);
            }
            if let Some(p) = dfs.stat(&part).await?.filter(|p| p.kind == Kind::File)
                && p.size < s.size
            {
                offset = p.size;
            }
        }
        let mut reader = sfs.open_read(src, offset).await?;
        let mut writer = dfs.open_write(&part, offset).await?;
        self.emit(QueueEvent::Started {
            id: ctx.id,
            file: src.to_string(),
            total: Some(s.size),
        });
        let mut done = offset;
        let mut last = Instant::now();
        let mut buf = vec![0u8; CHUNK];
        let copied: Result<()> = async {
            loop {
                self.gate(ctx).await?;
                let n = reader.read(&mut buf).await?;
                if n == 0 {
                    return Ok(());
                }
                self.throttle(n).await;
                writer.write_all(&buf[..n]).await?;
                done += n as u64;
                if last.elapsed() >= PROGRESS_EVERY {
                    last = Instant::now();
                    self.emit(QueueEvent::Progress {
                        id: ctx.id,
                        file: src.to_string(),
                        done,
                    });
                }
            }
        }
        .await;
        // Vaciar siempre, para que lo escrito hasta aquí sirva de base a una reanudación.
        let closed = async {
            writer.flush().await?;
            writer.shutdown().await
        }
        .await;
        copied?;
        closed?;
        self.emit(QueueEvent::Progress {
            id: ctx.id,
            file: src.to_string(),
            done,
        });
        if let (true, Some(m)) = (job.preserve_mtime, s.mtime) {
            dfs.set_mtime(&part, m).await?;
        }
        if let Some(mode) = s.mode {
            // Sin escritura para grupo/otros ni setuid/setgid/sticky.
            dfs.set_mode(&part, mode & 0o755).await?;
        }
        if dfs.rename(&part, dst).await.is_err() {
            // Algunos servidores (SFTP) no sobrescriben en `rename`.
            dfs.remove_file(dst).await?;
            dfs.rename(&part, dst).await?;
        }
        Ok(done - offset)
    }

    async fn exec(&self, ctx: &Ctx, job: &Job) -> Result<u64> {
        let dir = job.direction;
        let mut bytes = 0;
        let mut stack = vec![(job.src.clone(), job.dst.clone())];
        while let Some((s, d)) = stack.pop() {
            let st = self
                .retry(ctx, || async { self.ends(dir).0.stat(&s).await })
                .await?
                .ok_or_else(|| Error::NotFound(s.clone()))?;
            match st.kind {
                Kind::Dir => {
                    if !job.recursive {
                        return Err(Error::Protocol(format!("{s} es un directorio")));
                    }
                    if self
                        .retry(ctx, || async { self.ends(dir).1.stat(&d).await })
                        .await?
                        .is_none()
                    {
                        self.retry(ctx, || async { self.ends(dir).1.mkdir(&d).await })
                            .await?;
                    }
                    let mut list = self
                        .retry(ctx, || async { self.ends(dir).0.list(&s).await })
                        .await?;
                    list.sort_by(|a, b| b.name.cmp(&a.name)); // pop → orden alfabético
                    for e in list {
                        if safe_name(&e.name) && matches!(e.kind, Kind::File | Kind::Dir) {
                            stack.push((join(&s, &e.name), join(&d, &e.name)));
                        }
                    }
                }
                Kind::File => {
                    match self
                        .retry(ctx, || self.copy_file(ctx, job, &s, &d, &st))
                        .await
                    {
                        Ok(n) => bytes += n,
                        Err(e) => {
                            // Cancelado o sin reintentos: el parcial no se deja huérfano.
                            let _ = self
                                .ends(dir)
                                .1
                                .remove_file(&format!("{d}{PART_SUFFIX}"))
                                .await;
                            return Err(e);
                        }
                    }
                }
                _ => {} // enlaces y especiales no se transfieren
            }
        }
        Ok(bytes)
    }

    async fn run(self: Arc<Self>, ctx: Ctx, job: Job) {
        let res = match self.slots.acquire().await {
            Ok(_permit) => match self.gate(&ctx).await {
                Ok(()) => self.exec(&ctx, &job).await,
                Err(e) => Err(e),
            },
            Err(_) => Err(Error::Cancelled),
        };
        match res {
            Ok(bytes) => {
                self.ok.fetch_add(1, SeqCst);
                self.emit(QueueEvent::Finished { id: ctx.id, bytes });
            }
            Err(Error::Cancelled) => self.emit(QueueEvent::Cancelled { id: ctx.id }),
            Err(e) => {
                self.failed.fetch_add(1, SeqCst);
                self.emit(QueueEvent::Failed {
                    id: ctx.id,
                    error: e.to_string(),
                    kind: e.kind(),
                });
            }
        }
        lock(&self.cancels).remove(&ctx.id);
        self.pending.send_modify(|n| *n -= 1);
    }
}

impl Queue {
    /// `local` y `remote` son los dos extremos. Hay que construirla dentro de un runtime
    /// de tokio (`push` lanza tareas).
    pub fn new(
        local: Arc<dyn RemoteFs>,
        remote: Arc<dyn RemoteFs>,
        opts: QueueOptions,
        events: UnboundedSender<QueueEvent>,
    ) -> Self {
        Self {
            inner: Arc::new(Inner {
                local,
                remote: Mutex::new(remote),
                reconnect: Mutex::new(None),
                slots: Semaphore::new(opts.parallel.max(1)),
                opts,
                events,
                paused: AtomicBool::new(false),
                cancels: Mutex::new(HashMap::new()),
                bucket: Mutex::new(Instant::now()),
                pending: watch::channel(0).0,
                ok: AtomicUsize::new(0),
                failed: AtomicUsize::new(0),
                next_id: AtomicU64::new(1),
            }),
        }
    }

    /// Activa la reconexión: ante un error de red/E/S se pide un `remote` nuevo con `f`.
    #[must_use]
    pub fn with_reconnect(self, f: Reconnect) -> Self {
        *lock(&self.inner.reconnect) = Some(f);
        self
    }

    pub fn push(&self, job: Job) -> JobId {
        let i = &self.inner;
        let id = i.next_id.fetch_add(1, SeqCst);
        let cancel = Arc::new(AtomicBool::new(false));
        lock(&i.cancels).insert(id, cancel.clone());
        i.pending.send_modify(|n| *n += 1);
        tokio::spawn(i.clone().run(Ctx { id, cancel }, job));
        id
    }

    /// Con `true`, los trabajos en curso se detienen en el siguiente bloque.
    pub fn pause(&self, paused: bool) {
        self.inner.paused.store(paused, SeqCst);
    }

    pub fn cancel(&self, id: JobId) {
        if let Some(c) = lock(&self.inner.cancels).get(&id) {
            c.store(true, SeqCst);
        }
    }

    /// Espera a que se vacíe la cola. Devuelve (correctos, fallidos) acumulados.
    pub async fn wait(&self) -> (usize, usize) {
        let mut rx = self.inner.pending.subscribe();
        let _ = rx.wait_for(|n| *n == 0).await;
        (self.inner.ok.load(SeqCst), self.inner.failed.load(SeqCst))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transport_errors_are_retryable() {
        use russh_sftp::client::error::Error as E;
        assert!(retryable(&crate::sftp::map_err(E::Timeout)));
        assert!(retryable(&crate::sftp::map_err(E::IO("roto".into()))));
        assert!(!retryable(&Error::NotFound("x".into())));
    }
}
