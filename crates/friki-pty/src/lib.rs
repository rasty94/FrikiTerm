//! Shell local en un pseudoterminal (TODO.md §4).

use std::io::{self, Read, Write};
use std::sync::{Arc, Mutex};

use friki_core::transport::{Stream, StreamEvent, StreamReader, StreamWriter};
use portable_pty::{ChildKiller, CommandBuilder, MasterPty, PtySize, native_pty_system};
use tokio::sync::mpsc;

fn err(e: impl std::fmt::Display) -> io::Error {
    io::Error::other(e.to_string())
}

fn size(cols: u16, rows: u16) -> PtySize {
    PtySize {
        rows,
        cols,
        pixel_width: 0,
        pixel_height: 0,
    }
}

/// Las escrituras solo se encolan; un hilo propio las vuelca al PTY, así un hijo que
/// no lee no puede congelar a quien llama.
struct Writer {
    /// `None` cuando el hijo ya terminó y se cerró el PTY.
    master: Arc<Mutex<Option<Box<dyn MasterPty + Send>>>>,
    queue: std::sync::mpsc::SyncSender<Vec<u8>>,
}

#[async_trait::async_trait]
impl StreamWriter for Writer {
    async fn write(&self, data: &[u8]) -> io::Result<()> {
        use std::sync::mpsc::TrySendError;
        self.queue.try_send(data.to_vec()).map_err(|e| match e {
            TrySendError::Full(_) => {
                io::Error::new(io::ErrorKind::WouldBlock, "cola de escritura llena")
            }
            TrySendError::Disconnected(_) => {
                io::Error::new(io::ErrorKind::BrokenPipe, "PTY cerrado")
            }
        })
    }

    async fn resize(&self, cols: u16, rows: u16) -> io::Result<()> {
        let m = self.master.lock().map_err(|_| err("PTY envenenado"))?;
        match m.as_ref() {
            Some(m) => m.resize(size(cols, rows)).map_err(err),
            None => Ok(()),
        }
    }
}

/// Al soltar el lector se mata el proceso hijo (no deja shells huérfanas).
struct Reader {
    rx: mpsc::Receiver<StreamEvent>,
    killer: Box<dyn ChildKiller + Send + Sync>,
}

impl Drop for Reader {
    fn drop(&mut self) {
        let _ = self.killer.kill();
    }
}

#[async_trait::async_trait]
impl StreamReader for Reader {
    async fn next_event(&mut self) -> Option<StreamEvent> {
        self.rx.recv().await
    }
}

fn default_shell() -> String {
    if cfg!(windows) {
        std::env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".into())
    } else {
        std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into())
    }
}

/// Lanza `command` (o `$SHELL` / `%COMSPEC%` si es `None`) en un PTY con `TERM=term`.
/// El fin del proceso produce `StreamEvent::Exit(código)`.
pub async fn spawn(
    command: Option<&str>,
    term: &str,
    cols: u16,
    rows: u16,
) -> std::io::Result<Stream> {
    let mut cmd = match command {
        None => CommandBuilder::new(default_shell()),
        Some(c) if cfg!(windows) => {
            let mut b = CommandBuilder::new(default_shell());
            b.args(["/c", c]);
            b
        }
        Some(c) => {
            let mut b = CommandBuilder::new("/bin/sh");
            b.args(["-c", c]);
            b
        }
    };
    cmd.env("TERM", term);

    let pair = native_pty_system().openpty(size(cols, rows)).map_err(err)?;
    let mut child = pair.slave.spawn_command(cmd).map_err(err)?;
    // El extremo esclavo debe cerrarse aquí para que el lector vea EOF al terminar el hijo.
    drop(pair.slave);
    let mut reader = pair.master.try_clone_reader().map_err(err)?;
    let mut input = pair.master.take_writer().map_err(err)?;
    let (wq, wrx) = std::sync::mpsc::sync_channel::<Vec<u8>>(256);
    std::thread::spawn(move || {
        while let Ok(d) = wrx.recv() {
            if input.write_all(&d).and_then(|_| input.flush()).is_err() {
                return;
            }
        }
    });
    let killer = child.clone_killer();
    let master = Arc::new(Mutex::new(Some(pair.master)));

    // Hilo que espera al hijo. Al terminar cierra el PTY: en Windows (ConPTY) el lector
    // no recibe EOF solo porque el hijo acabe; hay que cerrar la pseudoconsola.
    let waiter_master = Arc::clone(&master);
    let waiter = std::thread::spawn(move || {
        let ev = match child.wait() {
            Ok(st) => StreamEvent::Exit(i32::try_from(st.exit_code()).unwrap_or(i32::MAX)),
            Err(e) => StreamEvent::Lost(e.to_string()),
        };
        if cfg!(windows) {
            // ponytail: en Unix se deja abierto hasta drenar la salida (EOF llega solo).
            if let Ok(mut m) = waiter_master.lock() {
                m.take();
            }
        }
        ev
    });

    let (tx, rx) = mpsc::channel(64);
    // Segundo "matador" para el hilo lector (el primero lo guarda `Reader` para su `Drop`).
    let reader_killer = Mutex::new(killer.clone_killer());
    std::thread::spawn(move || {
        let mut buf = [0u8; 4096];
        loop {
            match reader.read(&mut buf) {
                // EOF o EIO (Linux) = el proceso cerró el PTY
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if tx
                        .blocking_send(StreamEvent::Data(buf[..n].to_vec()))
                        .is_err()
                    {
                        // Nadie escucha: matar al hijo; el hilo `waiter` lo recoge (sin zombi).
                        let _ = reader_killer.lock().map(|mut k| k.kill());
                        let _ = waiter.join();
                        return;
                    }
                }
            }
        }
        let ev = waiter
            .join()
            .unwrap_or_else(|_| StreamEvent::Lost("hilo del proceso terminó con pánico".into()));
        let _ = tx.blocking_send(ev);
    });

    Ok((
        Arc::new(Writer { master, queue: wq }),
        Box::new(Reader { rx, killer }),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// Lee hasta el evento final; devuelve (texto, código).
    async fn collect(r: &mut dyn StreamReader) -> (String, Option<i32>) {
        let mut out = Vec::new();
        let fin = tokio::time::timeout(Duration::from_secs(15), async {
            while let Some(ev) = r.next_event().await {
                match ev {
                    StreamEvent::Data(d) => out.extend(d),
                    StreamEvent::Exit(c) => return Some(c),
                    _ => return None,
                }
            }
            None
        })
        .await
        .unwrap_or(None);
        (String::from_utf8_lossy(&out).into_owned(), fin)
    }

    #[tokio::test]
    async fn echo_then_exit_0() {
        let (_w, mut r) = spawn(Some("echo hola"), "xterm", 80, 24).await.unwrap();
        let (text, code) = collect(&mut *r).await;
        assert!(text.contains("hola"), "{text:?}");
        assert_eq!(code, Some(0));
    }

    #[tokio::test]
    async fn exit_code_is_reported() {
        let (_w, mut r) = spawn(Some("exit 3"), "xterm", 80, 24).await.unwrap();
        assert_eq!(collect(&mut *r).await.1, Some(3));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn term_size_and_resize() {
        let (w, mut r) = spawn(
            Some("echo $TERM; stty size; sleep 1; stty size"),
            "vt100",
            100,
            30,
        )
        .await
        .unwrap();
        tokio::time::sleep(Duration::from_millis(300)).await;
        w.resize(120, 40).await.unwrap();
        let (text, code) = collect(&mut *r).await;
        assert!(text.contains("vt100"), "{text:?}");
        assert!(text.contains("30 100"), "{text:?}");
        assert!(text.contains("40 120"), "{text:?}");
        assert_eq!(code, Some(0));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn write_never_blocks_when_child_does_not_read() {
        let (w, _r) = spawn(Some("stty raw; sleep 4"), "xterm", 80, 24)
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(300)).await;
        let t = std::time::Instant::now();
        let _ = w.write(&vec![b'x'; 200_000]).await;
        assert!(
            t.elapsed() < Duration::from_millis(100),
            "{:?}",
            t.elapsed()
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn write_reaches_process() {
        let (w, mut r) = spawn(Some("read x; echo got:$x"), "xterm", 80, 24)
            .await
            .unwrap();
        w.write(b"abc\n").await.unwrap();
        let (text, code) = collect(&mut *r).await;
        assert!(text.contains("got:abc"), "{text:?}");
        assert_eq!(code, Some(0));
    }
}
