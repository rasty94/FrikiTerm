//! Sesión TN3270 asíncrona: un flujo (TCP o duplex de pruebas) más el `Client` síncrono.

use std::io;
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;

use crate::client::{Client, Tn3270Options};
use crate::keys::Key;

pub struct Session<S> {
    io: S,
    pub client: Client,
}

impl<S: AsyncRead + AsyncWrite + Unpin> Session<S> {
    pub fn new(io: S, opts: &Tn3270Options) -> Self {
        Self {
            io,
            client: Client::new(opts),
        }
    }

    /// Lee un bloque del servidor, lo procesa y envía las respuestas. `Ok(false)` = cierre.
    pub async fn pump(&mut self) -> io::Result<bool> {
        let mut buf = [0u8; 4096];
        let n = self.io.read(&mut buf).await?;
        if n == 0 {
            return Ok(false);
        }
        let out = self.client.feed(&buf[..n]);
        if !out.to_server.is_empty() {
            self.io.write_all(&out.to_server).await?;
        }
        Ok(true)
    }

    /// Envía una tecla del usuario al servidor (si el teclado lo permite).
    pub async fn key(&mut self, k: Key) -> io::Result<()> {
        let out = self.client.key(k);
        if !out.to_server.is_empty() {
            self.io.write_all(&out.to_server).await?;
        }
        Ok(())
    }
}

/// Conexión TCP en claro con tiempo límite de establecimiento.
pub async fn connect_tcp(
    host: &str,
    port: u16,
    opts: &Tn3270Options,
) -> io::Result<Session<TcpStream>> {
    let secs = Duration::from_secs(opts.connect_timeout_secs);
    let tcp = tokio::time::timeout(secs, TcpStream::connect((host, port)))
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "tiempo agotado al conectar"))??;
    Ok(Session::new(tcp, opts))
}
