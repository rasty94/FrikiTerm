//! Redirecciones de puertos: local (`-L`), remota (`-R`) y dinámica SOCKS5 (`-D`).

use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::{Arc, PoisonError};
use std::time::Duration;

use friki_core::config::Forward;
use russh::client::Handle;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;

use crate::session::{Handler, Info};
use crate::{Error, Result};

/// Plazo para que un cliente SOCKS5 envíe su petición.
const SOCKS_TIMEOUT: Duration = Duration::from_secs(10);

pub(crate) enum Active {
    /// Escucha local (`-L`, `-D`): se detiene abortando la tarea.
    Task(JoinHandle<()>),
    /// Escucha en el servidor (`-R`): se detiene con `cancel-tcpip-forward`.
    Remote { addr: String, port: u32 },
}

impl Drop for Active {
    fn drop(&mut self) {
        if let Active::Task(t) = self {
            t.abort();
        }
    }
}

/// `bind` vacío = solo loopback. Devuelve (dirección, ¿expuesta a la red?).
fn local_bind(bind: &str) -> (String, bool) {
    match bind {
        "" => ("127.0.0.1".into(), false),
        "*" => ("0.0.0.0".into(), true),
        b => (
            b.to_string(),
            matches!(b, "0.0.0.0" | "::" | "[::]")
                || b.parse::<IpAddr>().is_ok_and(|i| i.is_unspecified()),
        ),
    }
}

fn exposed_warning(kind: &str, bind: &str, port: u16) -> String {
    format!(
        "redirección {kind} escuchando en {bind}:{port}: queda accesible desde la red, no solo desde esta máquina"
    )
}

pub(crate) async fn start(
    handle: &Arc<Handle<Handler>>,
    info: &Arc<Info>,
    fwd: &Forward,
) -> Result<(SocketAddr, Active, Option<String>)> {
    match fwd {
        Forward::Local {
            bind,
            port,
            host,
            host_port,
        } => {
            let (listener, listen, warn) = listen("local", bind, *port).await?;
            let (h, dest) = (handle.clone(), (host.clone(), *host_port));
            let task = tokio::spawn(accept_loop(listener, move |s, peer| {
                let (h, dest) = (h.clone(), dest.clone());
                async move { pipe(&h, s, peer, &dest.0, dest.1).await }
            }));
            Ok((listen, Active::Task(task), warn))
        }
        Forward::Dynamic { bind, port } => {
            let (listener, listen, warn) = listen("dinámica", bind, *port).await?;
            let h = handle.clone();
            let task = tokio::spawn(accept_loop(listener, move |mut s, peer| {
                let h = h.clone();
                async move {
                    let dest = match tokio::time::timeout(SOCKS_TIMEOUT, socks5(&mut s)).await {
                        Ok(Ok(d)) => d,
                        _ => return,
                    };
                    pipe_socks(&h, s, peer, dest).await;
                }
            }));
            Ok((listen, Active::Task(task), warn))
        }
        Forward::Remote {
            bind,
            port,
            host,
            host_port,
        } => {
            let (addr, exposed) = match bind.as_str() {
                "" => ("127.0.0.1".to_string(), false),
                "*" => (String::new(), true),
                b => local_bind(b),
            };
            let dest = (host.clone(), *host_port);
            let lock = || {
                info.remote_forwards
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
            };
            if *port != 0 {
                lock().insert(u32::from(*port), dest.clone());
            }
            let real = match handle.tcpip_forward(addr.clone(), u32::from(*port)).await {
                Ok(p) => {
                    if p == 0 {
                        u32::from(*port)
                    } else {
                        p
                    }
                }
                Err(e) => {
                    lock().remove(&u32::from(*port));
                    return Err(Error::Other(format!(
                        "el servidor rechazó la redirección remota: {e}"
                    )));
                }
            };
            if *port == 0 {
                lock().insert(real, dest);
            }
            let ip = addr.parse().unwrap_or(IpAddr::V4(Ipv4Addr::LOCALHOST));
            let listen = SocketAddr::new(ip, u16::try_from(real).unwrap_or(0));
            let warn = exposed.then(|| exposed_warning("remota", bind, listen.port()));
            Ok((listen, Active::Remote { addr, port: real }, warn))
        }
    }
}

async fn listen(
    kind: &str,
    bind: &str,
    port: u16,
) -> Result<(TcpListener, SocketAddr, Option<String>)> {
    let (addr, exposed) = local_bind(bind);
    let l = TcpListener::bind((addr.as_str(), port))
        .await
        .map_err(|e| Error::Other(format!("escuchar en {addr}:{port}: {e}")))?;
    let listen = l
        .local_addr()
        .map_err(|e| Error::Other(format!("dirección de escucha: {e}")))?;
    let warn = exposed.then(|| exposed_warning(kind, &addr, listen.port()));
    Ok((l, listen, warn))
}

async fn accept_loop<F, Fut>(listener: TcpListener, on_conn: F)
where
    F: Fn(TcpStream, SocketAddr) -> Fut,
    Fut: Future<Output = ()> + Send + 'static,
{
    while let Ok((s, peer)) = listener.accept().await {
        tokio::spawn(on_conn(s, peer));
    }
}

async fn pipe(h: &Handle<Handler>, mut s: TcpStream, peer: SocketAddr, host: &str, port: u16) {
    let Ok(ch) = h
        .channel_open_direct_tcpip(host, port.into(), peer.ip().to_string(), peer.port().into())
        .await
    else {
        return;
    };
    let mut c = ch.into_stream();
    let _ = tokio::io::copy_bidirectional(&mut s, &mut c).await;
}

async fn pipe_socks(
    h: &Handle<Handler>,
    mut s: TcpStream,
    peer: SocketAddr,
    (host, port): (String, u16),
) {
    match h
        .channel_open_direct_tcpip(host, port.into(), peer.ip().to_string(), peer.port().into())
        .await
    {
        Ok(ch) => {
            if s.write_all(&socks_reply(0)).await.is_err() {
                return;
            }
            let mut c = ch.into_stream();
            let _ = tokio::io::copy_bidirectional(&mut s, &mut c).await;
        }
        Err(_) => {
            let _ = s.write_all(&socks_reply(5)).await;
        }
    }
}

fn socks_reply(code: u8) -> [u8; 10] {
    [5, code, 0, 1, 0, 0, 0, 0, 0, 0]
}

fn proto(msg: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg.to_string())
}

/// SOCKS5 sin autenticación, solo `CONNECT` (IPv4, IPv6 o dominio). Responde los
/// rechazos al cliente; la respuesta de éxito la da quien abre el canal.
pub(crate) async fn socks5<S: AsyncRead + AsyncWrite + Unpin>(
    s: &mut S,
) -> io::Result<(String, u16)> {
    let mut head = [0u8; 2];
    s.read_exact(&mut head).await?;
    if head[0] != 5 {
        return Err(proto("no es SOCKS5"));
    }
    let mut methods = vec![0u8; usize::from(head[1])];
    s.read_exact(&mut methods).await?;
    if !methods.contains(&0) {
        s.write_all(&[5, 0xFF]).await?;
        return Err(proto("el cliente exige autenticación"));
    }
    s.write_all(&[5, 0]).await?;

    let mut req = [0u8; 4];
    s.read_exact(&mut req).await?;
    if req[0] != 5 {
        return Err(proto("petición SOCKS5 inválida"));
    }
    if req[1] != 1 {
        s.write_all(&socks_reply(7)).await?; // orden no soportada
        return Err(proto("solo CONNECT"));
    }
    let host = match req[3] {
        1 => {
            let mut a = [0u8; 4];
            s.read_exact(&mut a).await?;
            Ipv4Addr::from(a).to_string()
        }
        4 => {
            let mut a = [0u8; 16];
            s.read_exact(&mut a).await?;
            Ipv6Addr::from(a).to_string()
        }
        3 => {
            let mut len = [0u8; 1];
            s.read_exact(&mut len).await?;
            let mut name = vec![0u8; usize::from(len[0])];
            s.read_exact(&mut name).await?;
            match String::from_utf8(name) {
                Ok(n) if !n.is_empty() => n,
                _ => {
                    s.write_all(&socks_reply(8)).await?;
                    return Err(proto("dominio inválido"));
                }
            }
        }
        _ => {
            s.write_all(&socks_reply(8)).await?; // tipo de dirección no soportado
            return Err(proto("tipo de dirección no soportado"));
        }
    };
    let mut p = [0u8; 2];
    s.read_exact(&mut p).await?;
    Ok((host, u16::from_be_bytes(p)))
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn run(bytes: &[u8]) -> (io::Result<(String, u16)>, Vec<u8>) {
        let (mut client, mut server) = tokio::io::duplex(1024);
        client.write_all(bytes).await.expect("escribir");
        let r = socks5(&mut server).await;
        drop(server);
        let mut out = Vec::new();
        client.read_to_end(&mut out).await.expect("leer");
        (r, out)
    }

    #[tokio::test]
    async fn socks5_connect_ipv4_ipv6_y_dominio() {
        let (r, out) = run(&[5, 1, 0, 5, 1, 0, 1, 10, 0, 0, 7, 0, 80]).await;
        assert_eq!(r.expect("ipv4"), ("10.0.0.7".into(), 80));
        assert_eq!(out, [5, 0]);

        let mut v6 = vec![5, 1, 0, 5, 1, 0, 4];
        v6.extend(Ipv6Addr::LOCALHOST.octets());
        v6.extend([0x01, 0xBB]);
        assert_eq!(run(&v6).await.0.expect("ipv6"), ("::1".into(), 443));

        let mut dom = vec![5, 1, 0, 5, 1, 0, 3, 9];
        dom.extend(b"localhost");
        dom.extend([0, 22]);
        assert_eq!(
            run(&dom).await.0.expect("dominio"),
            ("localhost".into(), 22)
        );
    }

    #[tokio::test]
    async fn socks5_rechaza_otros_comandos_y_autenticacion() {
        // BIND (2)
        let (r, out) = run(&[5, 1, 0, 5, 2, 0, 1, 1, 1, 1, 1, 0, 1]).await;
        assert!(r.is_err());
        assert_eq!(out[2..4], [5, 7]);
        // Solo usuario/contraseña (2)
        let (r, out) = run(&[5, 1, 2]).await;
        assert!(r.is_err());
        assert_eq!(out, [5, 0xFF]);
    }
}
