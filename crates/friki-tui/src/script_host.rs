//! `ScriptHost` sobre un panel. El bucle de la app alimenta un `Shared` (bytes recibidos,
//! foto de la pantalla, escritor del flujo) y el script, que corre en otra tarea, espera
//! sobre él. `connect`, `disconnect` y `message` se piden al bucle por `HostReq`.
//!
//! El búfer de recepción conserva lo llegado desde el inicio del script o desde la última
//! coincidencia (un `send` seguido de `wait_for_strings` no pierde la respuesta si llega
//! antes de que empiece la espera). Una coincidencia consume hasta su final.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use friki_core::transport::StreamWriter;
use friki_script::ScriptHost;
use friki_term::Screen;
use tokio::sync::{Notify, oneshot};
use tokio::time::Instant;

/// Tope del búfer de recepción: se descarta lo más antiguo.
const MAX_BUF: usize = 256 * 1024;

/// Peticiones del script al bucle de la app.
pub enum HostReq {
    Message(String),
    Connect(String, oneshot::Sender<Result<(), String>>),
    Disconnect,
}

#[derive(Default)]
struct Inner {
    buf: Vec<u8>,
    screen: Vec<String>,
    cursor: (u16, u16),
    writer: Option<Arc<dyn StreamWriter>>,
}

#[derive(Default)]
pub struct Shared {
    inner: Mutex<Inner>,
    notify: Notify,
    cancelled: AtomicBool,
}

/// Filas de la pantalla como texto (sin espacios finales).
fn rows_text(s: &Screen) -> Vec<String> {
    let cols = usize::from(s.cols.max(1));
    s.cells
        .chunks(cols)
        .map(|r| {
            let line: String = r
                .iter()
                .map(|c| {
                    if c.text.is_empty() {
                        " "
                    } else {
                        c.text.as_str()
                    }
                })
                .collect();
            line.trim_end().to_string()
        })
        .collect()
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn push(&self, data: &[u8]) {
        {
            let mut g = self.lock();
            g.buf.extend_from_slice(data);
            if g.buf.len() > MAX_BUF {
                let cut = g.buf.len() - MAX_BUF;
                g.buf.drain(..cut);
            }
        }
        self.notify.notify_waiters();
    }

    pub fn set_screen(&self, s: &Screen) {
        let mut g = self.lock();
        g.screen = rows_text(s);
        g.cursor = s.cursor;
    }

    /// `None` cuando el panel ya no tiene sesión.
    pub fn set_writer(&self, w: Option<Arc<dyn StreamWriter>>) {
        self.lock().writer = w;
        self.notify.notify_waiters();
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
        self.notify.notify_waiters();
    }

    fn check(&self) -> Result<(), String> {
        if self.cancelled.load(Ordering::SeqCst) {
            Err("script cancelado".into())
        } else {
            Ok(())
        }
    }
}

/// (índice de la cadena, inicio, fin) de la primera aparición en el búfer; gana la que
/// empieza antes y, a igual inicio, la de menor índice.
fn find(buf: &[u8], needles: &[String]) -> Option<(usize, usize, usize)> {
    needles
        .iter()
        .enumerate()
        .filter(|(_, n)| !n.is_empty())
        .filter_map(|(i, n)| {
            let n = n.as_bytes();
            buf.windows(n.len())
                .position(|w| w == n)
                .map(|s| (i, s, s + n.len()))
        })
        .min_by_key(|&(i, s, _)| (s, i))
}

pub struct PaneHost {
    shared: Arc<Shared>,
    req: Arc<dyn Fn(HostReq) -> bool + Send + Sync>,
}

impl PaneHost {
    pub fn new(shared: Arc<Shared>, req: Arc<dyn Fn(HostReq) -> bool + Send + Sync>) -> Self {
        Self { shared, req }
    }

    /// Espera una coincidencia y devuelve (índice, texto anterior a ella).
    async fn wait(
        &self,
        needles: &[String],
        timeout: Duration,
    ) -> Result<Option<(usize, Vec<u8>)>, String> {
        let deadline = Instant::now() + timeout;
        loop {
            // Se crea antes de mirar el búfer: no se pierde ningún aviso intermedio.
            let notified = self.shared.notify.notified();
            self.shared.check()?;
            {
                let mut g = self.shared.lock();
                if let Some((i, s, e)) = find(&g.buf, needles) {
                    let before = g.buf[..s].to_vec();
                    g.buf.drain(..e);
                    return Ok(Some((i, before)));
                }
                if g.writer.is_none() {
                    return Err("el panel no está conectado".into());
                }
            }
            if tokio::time::timeout_at(deadline, notified).await.is_err() {
                return Ok(None);
            }
        }
    }
}

#[async_trait::async_trait]
impl ScriptHost for PaneHost {
    async fn send(&self, text: &str) -> Result<(), String> {
        self.shared.check()?;
        let w = self.shared.lock().writer.clone();
        let w = w.ok_or("el panel no está conectado")?;
        w.write(text.as_bytes()).await.map_err(|e| e.to_string())
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
        Ok(self
            .wait(until, timeout)
            .await?
            .map(|(_, before)| String::from_utf8_lossy(&before).into_owned()))
    }

    fn screen_text(&self) -> Vec<String> {
        self.shared.lock().screen.clone()
    }

    fn cursor(&self) -> (u16, u16) {
        let (r, c) = self.shared.lock().cursor;
        (r.saturating_add(1), c.saturating_add(1))
    }

    async fn connect(&self, target: &str) -> Result<(), String> {
        self.shared.check()?;
        let (tx, rx) = oneshot::channel();
        if !(self.req)(HostReq::Connect(target.to_string(), tx)) {
            return Err("la aplicación se está cerrando".into());
        }
        rx.await.map_err(|_| "conexión abandonada".to_string())?
    }

    async fn disconnect(&self) -> Result<(), String> {
        if (self.req)(HostReq::Disconnect) {
            Ok(())
        } else {
            Err("la aplicación se está cerrando".into())
        }
    }

    fn connected(&self) -> bool {
        self.shared.lock().writer.is_some()
    }

    fn message(&self, text: &str) {
        let _ = (self.req)(HostReq::Message(text.to_string()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io;

    struct Rec(Mutex<Vec<u8>>);

    #[async_trait::async_trait]
    impl StreamWriter for Rec {
        async fn write(&self, data: &[u8]) -> io::Result<()> {
            self.0
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .extend_from_slice(data);
            Ok(())
        }
        async fn resize(&self, _: u16, _: u16) -> io::Result<()> {
            Ok(())
        }
    }

    type Fixture = (PaneHost, Arc<Shared>, Arc<Rec>, Arc<Mutex<Vec<String>>>);

    fn host() -> Fixture {
        let shared = Arc::new(Shared::default());
        let rec = Arc::new(Rec(Mutex::new(vec![])));
        shared.set_writer(Some(rec.clone()));
        let msgs = Arc::new(Mutex::new(vec![]));
        let m = msgs.clone();
        let req: Arc<dyn Fn(HostReq) -> bool + Send + Sync> = Arc::new(move |r| {
            if let HostReq::Message(t) = r {
                m.lock().unwrap_or_else(PoisonError::into_inner).push(t);
            }
            true
        });
        (PaneHost::new(shared.clone(), req), shared, rec, msgs)
    }

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| (*x).to_string()).collect()
    }

    const T: Duration = Duration::from_secs(2);

    #[tokio::test]
    async fn send_writes_to_the_stream_and_message_goes_to_the_loop() {
        let (h, _, rec, msgs) = host();
        h.send("ls\n").await.expect("send");
        assert_eq!(*rec.0.lock().expect("rec"), b"ls\n");
        h.message("hola");
        assert_eq!(*msgs.lock().expect("msgs"), vec!["hola".to_string()]);
    }

    #[tokio::test]
    async fn wait_matches_data_that_arrives_later_and_consumes_it() {
        let (h, sh, ..) = host();
        sh.push(b"viejo login: ");
        let w = tokio::spawn(async move {
            let a = h.wait_for_strings(&s(&["nada", "login:"]), T).await;
            let b = h.read_string(&s(&["$ "]), T).await;
            (a, b)
        });
        tokio::time::sleep(Duration::from_millis(30)).await;
        sh.push(b"user\r\nok");
        sh.push(b" $ resto");
        let (a, b) = w.await.expect("tarea");
        assert_eq!(a, Ok(Some(1)));
        assert_eq!(b, Ok(Some(" user\r\nok ".to_string())));
    }

    #[tokio::test]
    async fn match_split_across_chunks_and_earliest_wins() {
        let (h, sh, ..) = host();
        sh.push(b"ab");
        sh.push(b"cd xx");
        let r = h.wait_for_strings(&s(&["xx", "bc"]), T).await;
        assert_eq!(r, Ok(Some(1)));
    }

    #[tokio::test]
    async fn timeout_returns_none_and_keeps_the_buffer() {
        let (h, sh, ..) = host();
        sh.push(b"datos");
        let r = h
            .wait_for_strings(&s(&["zzz"]), Duration::from_millis(30))
            .await;
        assert_eq!(r, Ok(None));
        assert_eq!(h.read_string(&s(&["s"]), T).await, Ok(Some("dato".into())));
    }

    #[tokio::test]
    async fn cancel_and_closed_pane_are_errors() {
        let (h, sh, ..) = host();
        let h = Arc::new(h);
        let h2 = h.clone();
        let w = tokio::spawn(async move { h2.wait_for_strings(&s(&["x"]), T).await });
        tokio::time::sleep(Duration::from_millis(30)).await;
        sh.cancel();
        assert!(w.await.expect("tarea").is_err());
        assert!(h.send("a").await.is_err());
        let (h, sh, ..) = host();
        sh.set_writer(None);
        assert!(!h.connected());
        assert!(h.wait_for_strings(&s(&["x"]), T).await.is_err());
    }

    #[test]
    fn screen_and_cursor_are_one_based() {
        let (h, sh, ..) = host();
        let mut t = friki_term::Terminal::new(3, 10, 0);
        t.process(b"hola\r\nmundo");
        sh.set_screen(&t.screen());
        assert_eq!(h.screen_text(), vec!["hola", "mundo", ""]);
        assert_eq!(h.cursor(), (2, 6));
    }
}
