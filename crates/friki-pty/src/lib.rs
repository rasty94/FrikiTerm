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

struct Writer {
    master: Mutex<Box<dyn MasterPty + Send>>,
    input: Arc<Mutex<Box<dyn Write + Send>>>,
}

#[async_trait::async_trait]
impl StreamWriter for Writer {
    async fn write(&self, data: &[u8]) -> io::Result<()> {
        let input = self.input.clone();
        let data = data.to_vec();
        tokio::task::spawn_blocking(move || {
            let mut w = input.lock().map_err(|_| err("PTY envenenado"))?;
            w.write_all(&data)?;
            w.flush()
        })
        .await
        .map_err(err)?
    }

    async fn resize(&self, cols: u16, rows: u16) -> io::Result<()> {
        let m = self.master.lock().map_err(|_| err("PTY envenenado"))?;
        m.resize(size(cols, rows)).map_err(err)
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
    let input = pair.master.take_writer().map_err(err)?;
    let killer = child.clone_killer();

    let (tx, rx) = mpsc::channel(64);
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
                        return;
                    }
                }
            }
        }
        let ev = match child.wait() {
            Ok(st) => StreamEvent::Exit(st.exit_code() as i32),
            Err(e) => StreamEvent::Lost(e.to_string()),
        };
        let _ = tx.blocking_send(ev);
    });

    Ok((
        Arc::new(Writer {
            master: Mutex::new(pair.master),
            input: Arc::new(Mutex::new(input)),
        }),
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
