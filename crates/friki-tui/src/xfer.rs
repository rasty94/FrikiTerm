//! Transferencias Zmodem sobre el flujo de un panel: el adaptador `PaneIo` convierte el
//! `Stream` del panel (bytes que llegan por un canal, escritura por `StreamWriter`) en
//! `AsyncRead + AsyncWrite`, que es lo que piden `friki_zmodem::receive` y `send`.

use std::future::Future;
use std::io;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use friki_core::transport::StreamWriter;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::sync::mpsc;

/// Secuencia de cancelación Zmodem: cinco CAN.
pub const CANCEL: [u8; 5] = [0x18; 5];

type Pending = Pin<Box<dyn Future<Output = io::Result<usize>> + Send>>;

/// Lado de lectura: los `StreamEvent::Data` que el bucle desvía; lado de escritura: el
/// `StreamWriter` del panel. Un canal cerrado es EOF.
pub struct PaneIo {
    rx: mpsc::UnboundedReceiver<Vec<u8>>,
    buf: Vec<u8>,
    pos: usize,
    writer: Arc<dyn StreamWriter>,
    pending: Option<Pending>,
}

impl PaneIo {
    pub fn new(rx: mpsc::UnboundedReceiver<Vec<u8>>, writer: Arc<dyn StreamWriter>) -> Self {
        Self {
            rx,
            buf: Vec::new(),
            pos: 0,
            writer,
            pending: None,
        }
    }

    /// Lo recibido y no consumido (resto del búfer y cola del canal): al terminar la
    /// transferencia vuelve al emulador en vez de perderse.
    pub fn drain_rest(&mut self) -> Vec<u8> {
        let mut out = self.buf.split_off(self.pos.min(self.buf.len()));
        self.buf.clear();
        self.pos = 0;
        while let Ok(v) = self.rx.try_recv() {
            out.extend_from_slice(&v);
        }
        out
    }
}

impl AsyncRead for PaneIo {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        out: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = &mut *self;
        while this.pos >= this.buf.len() {
            match this.rx.poll_recv(cx) {
                Poll::Ready(Some(v)) => {
                    this.buf = v;
                    this.pos = 0;
                }
                Poll::Ready(None) => return Poll::Ready(Ok(())), // EOF
                Poll::Pending => return Poll::Pending,
            }
        }
        let n = out.remaining().min(this.buf.len() - this.pos);
        out.put_slice(&this.buf[this.pos..this.pos + n]);
        this.pos += n;
        Poll::Ready(Ok(()))
    }
}

impl AsyncWrite for PaneIo {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        data: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = &mut *self;
        if this.pending.is_none() {
            let (w, v) = (this.writer.clone(), data.to_vec());
            this.pending = Some(Box::pin(async move {
                w.write(&v).await?;
                Ok(v.len())
            }));
        }
        let Some(f) = this.pending.as_mut() else {
            return Poll::Pending; // inalcanzable: se acaba de crear
        };
        let r = f.as_mut().poll(cx);
        if r.is_ready() {
            this.pending = None;
        }
        r
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = &mut *self;
        if let Some(f) = this.pending.as_mut() {
            let r = f.as_mut().poll(cx);
            if r.is_pending() {
                return Poll::Pending;
            }
            this.pending = None;
            if let Poll::Ready(Err(e)) = r {
                return Poll::Ready(Err(e));
            }
        }
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.poll_flush(cx)
    }
}

/// Barra de progreso de texto: `[#####-----] 50%` (o solo bytes si no hay total).
pub fn bar(done: u64, total: Option<u64>, width: usize) -> String {
    match total.filter(|t| *t > 0) {
        Some(t) => {
            let pct = (done.min(t) * 100 / t) as usize;
            let on = width * pct / 100;
            format!("[{}{}] {pct}%", "#".repeat(on), "-".repeat(width - on))
        }
        None => format!("{done} bytes"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use friki_zmodem::{FileProtocol, Progress, receive, send};
    use tokio::io::{AsyncWriteExt, WriteHalf, split};
    use tokio::sync::Mutex;

    struct DuplexWriter(Mutex<WriteHalf<tokio::io::DuplexStream>>);

    #[async_trait::async_trait]
    impl StreamWriter for DuplexWriter {
        async fn write(&self, data: &[u8]) -> io::Result<()> {
            self.0.lock().await.write_all(data).await
        }
        async fn resize(&self, _: u16, _: u16) -> io::Result<()> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn zmodem_send_reaches_receive_through_pane_io() {
        use tokio::io::AsyncReadExt;
        let dir = std::env::temp_dir().join(format!("friki-tui-xfer-{}", std::process::id()));
        let (src, dst) = (dir.join("src"), dir.join("dst"));
        std::fs::create_dir_all(&src).expect("dir");
        let data: Vec<u8> = (0..70_000u32).map(|i| (i * 7 % 251) as u8).collect();
        let file = src.join("datos.bin");
        std::fs::write(&file, &data).expect("fichero");

        // El "remoto" (send) habla con el panel (PaneIo) a través de dos duplex.
        let (remote, panel) = tokio::io::duplex(1 << 16);
        let (mut panel_r, panel_w) = split(panel);
        let (tx, rx) = mpsc::unbounded_channel();
        let pump = tokio::spawn(async move {
            let mut b = vec![0u8; 4096];
            while let Ok(n) = panel_r.read(&mut b).await {
                if n == 0 || tx.send(b[..n].to_vec()).is_err() {
                    break;
                }
            }
        });
        let mut io = PaneIo::new(rx, Arc::new(DuplexWriter(Mutex::new(panel_w))));
        let mut remote = remote;
        let sender = tokio::spawn(async move {
            let mut p = |_: Progress| {};
            send(&mut remote, FileProtocol::Zmodem, &[file], &mut p).await
        });
        let mut seen = 0u64;
        let mut p = |x: Progress| seen = seen.max(x.done);
        let got = receive(&mut io, FileProtocol::Zmodem, &dst, &mut p).await;
        pump.abort();
        let _ = sender.await;
        let got = got.expect("recepción");
        assert_eq!(got.len(), 1);
        assert_eq!(std::fs::read(&got[0]).expect("leer"), data);
        assert!(seen > 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn drain_rest_returns_unread_and_queued_bytes() {
        use tokio::io::AsyncReadExt;
        let (tx, rx) = mpsc::unbounded_channel();
        let (_, w) = split(tokio::io::duplex(8).0);
        let mut io = PaneIo::new(rx, Arc::new(DuplexWriter(Mutex::new(w))));
        tx.send(b"abcdef".to_vec()).unwrap();
        tx.send(b"gh".to_vec()).unwrap();
        let mut two = [0u8; 2];
        io.read_exact(&mut two).await.unwrap();
        assert_eq!(&two, b"ab");
        assert_eq!(io.drain_rest(), b"cdefgh");
        assert!(io.drain_rest().is_empty());
    }

    #[test]
    fn progress_bar() {
        assert_eq!(bar(50, Some(100), 10), "[#####-----] 50%");
        assert_eq!(bar(7, None, 10), "7 bytes");
        assert_eq!(bar(300, Some(100), 4), "[####] 100%");
    }
}
