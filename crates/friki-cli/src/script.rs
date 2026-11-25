//! `friki script`: ejecuta un script Rhai sin TUI sobre un flujo de shell SSH.

use std::sync::atomic::{AtomicBool, Ordering::SeqCst};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use friki_cli::cli::ScriptArgs;
use friki_core::transport::{StreamEvent, StreamReader, StreamWriter};
use friki_script::{Limits, Permissions, ScriptHost};
use friki_term::Terminal;
use tokio::sync::Notify;

use crate::{connect_target, sanitize_remote};

const COLS: u16 = 80;
const ROWS: u16 = 24;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

struct Shared {
    term: Mutex<Terminal>,
    /// Bytes recibidos y aún no consumidos por `WaitFor*`/`ReadString`.
    buf: Mutex<Vec<u8>>,
    closed: AtomicBool,
    notify: Notify,
}

struct StreamHost {
    writer: Arc<dyn StreamWriter>,
    shared: Arc<Shared>,
}

impl StreamHost {
    /// Lanza la tarea que alimenta el emulador y el búfer con lo que llega del servidor.
    fn start(writer: Arc<dyn StreamWriter>, mut reader: Box<dyn StreamReader>) -> Arc<Self> {
        let shared = Arc::new(Shared {
            term: Mutex::new(Terminal::new(ROWS, COLS, 0)),
            buf: Mutex::new(Vec::new()),
            closed: AtomicBool::new(false),
            notify: Notify::new(),
        });
        let s = shared.clone();
        tokio::spawn(async move {
            // `Exit` no corta: aún pueden llegar datos hasta `Closed`/`Lost`.
            while let Some(ev) = reader.next_event().await {
                match ev {
                    StreamEvent::Data(d) => {
                        lock(&s.term).process(&d);
                        let mut buf = lock(&s.buf);
                        buf.extend_from_slice(&d);
                        cap_buf(&mut buf);
                        s.notify.notify_one();
                    }
                    StreamEvent::Exit(_) => {}
                    StreamEvent::Closed | StreamEvent::Lost(_) => break,
                }
            }
            s.closed.store(true, SeqCst);
            s.notify.notify_one();
        });
        Arc::new(Self { writer, shared })
    }

    /// Espera hasta `timeout` a que aparezca alguna cadena; consume el búfer hasta ella.
    /// Devuelve (índice, texto anterior a la coincidencia).
    async fn wait(
        &self,
        needles: &[String],
        timeout: Duration,
    ) -> Result<Option<(usize, String)>, String> {
        let deadline = Instant::now() + timeout;
        loop {
            // `closed` se lee antes que el búfer: así no se pierde lo llegado justo antes de cerrar.
            let closed = self.shared.closed.load(SeqCst);
            {
                let mut buf = lock(&self.shared.buf);
                if let Some((i, pos, len)) = earliest(&buf, needles) {
                    let before = String::from_utf8_lossy(&buf[..pos]).into_owned();
                    buf.drain(..pos + len);
                    return Ok(Some((i, before)));
                }
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if closed || left.is_zero() {
                return Ok(None);
            }
            let _ = tokio::time::timeout(left, self.shared.notify.notified()).await;
        }
    }
}

const MAX_BUF: usize = 256 * 1024;

/// Descarta lo más antiguo para que el búfer no pase de `MAX_BUF`.
fn cap_buf(buf: &mut Vec<u8>) {
    if buf.len() > MAX_BUF {
        buf.drain(..buf.len() - MAX_BUF);
    }
}

/// `crt.Quit(n)`: 0 → 0; 1..=249 → n; fuera de rango (los 250..=254 son de friki) → 254.
fn quit_code(n: i64) -> i32 {
    match i32::try_from(n) {
        Ok(c @ 0..=249) => c,
        _ => {
            eprintln!("friki: aviso: crt.Quit({n}) fuera de 0..=249; se usa 254");
            crate::EXIT_OTHER
        }
    }
}

/// Coincidencia más temprana: (índice de la cadena, posición, longitud).
fn earliest(buf: &[u8], needles: &[String]) -> Option<(usize, usize, usize)> {
    needles
        .iter()
        .enumerate()
        .filter(|(_, n)| !n.is_empty())
        .filter_map(|(i, n)| {
            let pos = buf.windows(n.len()).position(|w| w == n.as_bytes())?;
            Some((i, pos, n.len()))
        })
        .min_by_key(|(i, pos, _)| (*pos, *i))
}

#[async_trait::async_trait]
impl ScriptHost for StreamHost {
    async fn send(&self, text: &str) -> Result<(), String> {
        self.writer
            .write(text.as_bytes())
            .await
            .map_err(|e| e.to_string())
    }

    async fn wait_for_strings(
        &self,
        needles: &[String],
        timeout: Duration,
    ) -> Result<Option<usize>, String> {
        Ok(self.wait(needles, timeout).await?.map(|(i, _)| i))
    }

    async fn read_string(
        &self,
        until: &[String],
        timeout: Duration,
    ) -> Result<Option<String>, String> {
        Ok(self.wait(until, timeout).await?.map(|(_, text)| text))
    }

    fn screen_text(&self) -> Vec<String> {
        let screen = lock(&self.shared.term).screen();
        screen
            .cells
            .chunks(usize::from(screen.cols).max(1))
            .map(|row| {
                // ponytail: la segunda mitad de un carácter ancho cuenta como un espacio.
                row.iter()
                    .map(|c| if c.text.is_empty() { " " } else { &c.text })
                    .collect()
            })
            .collect()
    }

    fn cursor(&self) -> (u16, u16) {
        let (r, c) = lock(&self.shared.term).screen().cursor;
        (r + 1, c + 1)
    }

    async fn connect(&self, _target: &str) -> Result<(), String> {
        Err("crt.Session.Connect no está disponible en la CLI; usa --target".into())
    }

    async fn disconnect(&self) -> Result<(), String> {
        Err("crt.Session.Disconnect no está disponible en la CLI".into())
    }

    fn connected(&self) -> bool {
        !self.shared.closed.load(SeqCst)
    }

    fn message(&self, text: &str) {
        eprintln!("{}", sanitize_remote(text));
    }
}

/// Sin `--target`: el script corre sin pantalla y toda operación de sesión falla.
struct NoSession;

const NO_SESSION: &str = "sin sesión: usa --target";

#[async_trait::async_trait]
impl ScriptHost for NoSession {
    async fn send(&self, _: &str) -> Result<(), String> {
        Err(NO_SESSION.into())
    }
    async fn wait_for_strings(&self, _: &[String], _: Duration) -> Result<Option<usize>, String> {
        Err(NO_SESSION.into())
    }
    async fn read_string(&self, _: &[String], _: Duration) -> Result<Option<String>, String> {
        Err(NO_SESSION.into())
    }
    fn screen_text(&self) -> Vec<String> {
        Vec::new()
    }
    fn cursor(&self) -> (u16, u16) {
        (1, 1)
    }
    async fn connect(&self, _: &str) -> Result<(), String> {
        Err(NO_SESSION.into())
    }
    async fn disconnect(&self) -> Result<(), String> {
        Err(NO_SESSION.into())
    }
    fn connected(&self) -> bool {
        false
    }
    fn message(&self, text: &str) {
        eprintln!("{}", sanitize_remote(text));
    }
}

pub async fn run(args: ScriptArgs) -> anyhow::Result<i32> {
    let source = std::fs::read_to_string(&args.file)?;
    let perms = Permissions {
        read: args.allow_read,
        write: args.allow_write,
        connect: args.allow_connect,
    };
    let (host, conn): (Arc<dyn ScriptHost>, _) = match args.target {
        Some(t) => {
            let conn = connect_target(&t, args.conn).await?;
            let term = friki_term::Terminal::term_name(Default::default());
            let (w, r) = conn.open_shell_stream(term, COLS, ROWS).await?;
            (StreamHost::start(w, r), Some(conn))
        }
        None => (Arc::new(NoSession), None),
    };
    // Ctrl+C cancela el script (cooperativo: Sleep, esperas y contador de operaciones).
    let cancel = friki_script::CancelToken::new();
    let on_ctrl_c = cancel.clone();
    let watcher = tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            on_ctrl_c.cancel();
        }
    });
    let res = friki_script::run_cancellable(
        &source,
        &args.args,
        host,
        &perms,
        &Limits::default(),
        &cancel,
    )
    .await;
    watcher.abort();
    if let Some(c) = conn {
        let _ = c.disconnect().await;
    }
    Ok(quit_code(res?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quit_code_rangos() {
        assert_eq!(quit_code(0), 0);
        assert_eq!(quit_code(7), 7);
        assert_eq!(quit_code(249), 249);
        assert_eq!(quit_code(250), 254);
        assert_eq!(quit_code(300), 254);
        assert_eq!(quit_code(-1), 254);
        assert_eq!(quit_code(1 << 40), 254);
    }

    #[test]
    fn buffer_acotado() {
        let mut b = vec![1u8; MAX_BUF + 10];
        b.extend_from_slice(&[9, 9]);
        cap_buf(&mut b);
        assert_eq!(b.len(), MAX_BUF);
        assert_eq!(&b[MAX_BUF - 2..], &[9, 9]);
    }

    #[test]
    fn earliest_elige_la_primera_aparicion() {
        let n = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(earliest(b"xx ab cd", &n(&["cd", "ab"])), Some((1, 3, 2)));
        assert_eq!(earliest(b"xx", &n(&["cd", ""])), None);
    }
}
