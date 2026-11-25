//! `fwd` y los subcomandos que abren la TUI con un protocolo distinto de SSH.

use friki_cli::cli::{FwdArgs, SerialArgs, ShellArgs, TelnetArgs, TftpServerArgs};
use friki_core::config::{Forward, Protocol, SerialConfig, SessionConfig};

use crate::{CodedError, EXIT_NETWORK, UsageError, connect, load_target, sanitize_remote};

/// Servidor TFTP en primer plano hasta Ctrl+C.
pub async fn tftp_server(args: TftpServerArgs) -> anyhow::Result<i32> {
    if !args.root.is_dir() {
        return Err(UsageError(format!("{} no es un directorio", args.root.display())).into());
    }
    let net_err = |e: std::io::Error| CodedError(EXIT_NETWORK, format!("TFTP {}: {e}", args.bind));
    let sock = tokio::net::UdpSocket::bind(args.bind)
        .await
        .map_err(net_err)?;
    eprintln!(
        "TFTP en {} sirviendo {} ({})",
        args.bind,
        args.root.display(),
        if args.allow_write {
            "lectura y escritura"
        } else {
            "solo lectura"
        }
    );
    let cfg = friki_tftp::Config {
        root: args.root.clone(),
        allow_write: args.allow_write,
        overwrite: args.overwrite,
        log: |m| eprintln!("tftp: {}", sanitize_remote(m)),
    };
    tokio::select! {
        r = friki_tftp::serve(sock, cfg) => r.map_err(net_err)?,
        _ = tokio::signal::ctrl_c() => {}
    }
    Ok(0)
}

/// `host[:puerto]` o `[v6][:puerto]`. Un IPv6 sin corchetes no admite puerto.
pub fn split_host_port(s: &str) -> Option<(String, Option<u16>)> {
    let (host, port) = if let Some(rest) = s.strip_prefix('[') {
        let (h, after) = rest.split_once(']')?;
        let port = match after {
            "" => None,
            a => Some(a.strip_prefix(':')?),
        };
        (h, port)
    } else if s.matches(':').count() == 1 {
        let (h, p) = s.split_once(':')?;
        (h, Some(p))
    } else {
        (s, None)
    };
    if host.is_empty() || host.contains(char::is_whitespace) {
        return None;
    }
    let port = match port {
        None => None,
        Some(p) => Some(p.parse::<u16>().ok().filter(|&p| p != 0)?),
    };
    Some((host.to_string(), port))
}

fn bind_text(bind: &str) -> String {
    let b = if bind.is_empty() { "127.0.0.1" } else { bind };
    if b.contains(':') {
        format!("[{b}]")
    } else {
        b.to_string()
    }
}

fn host_text(host: &str) -> String {
    if host.contains(':') {
        format!("[{host}]")
    } else {
        host.to_string()
    }
}

/// Descripción de una redirección para el usuario (sintaxis de OpenSSH).
fn describe(f: &Forward) -> String {
    match f {
        Forward::Local {
            bind,
            port,
            host,
            host_port,
        } => format!(
            "-L {}:{port}:{}:{host_port}",
            bind_text(bind),
            host_text(host)
        ),
        Forward::Remote {
            bind,
            port,
            host,
            host_port,
        } => format!(
            "-R {}:{port}:{}:{host_port}",
            bind_text(bind),
            host_text(host)
        ),
        Forward::Dynamic { bind, port } => format!("-D {}:{port}", bind_text(bind)),
    }
}

/// Mantiene la conexión y las redirecciones hasta Ctrl+C.
pub async fn fwd(args: FwdArgs) -> anyhow::Result<i32> {
    let mut cfg = load_target(&args.target)?;
    args.conn.apply(&mut cfg);
    let forwards = std::mem::take(&mut cfg.forwards);
    if forwards.is_empty() {
        return Err(UsageError("fwd necesita al menos un -L, -R o -D".into()).into());
    }
    crate::sin_agente(&mut cfg);
    let conn = connect(&cfg).await?;
    for f in &forwards {
        let h = conn.start_forward(f).await?;
        println!("{} activa (escuchando en {})", describe(f), h.listen);
    }
    for aviso in conn.warnings() {
        eprintln!("friki: aviso: {}", sanitize_remote(&aviso));
    }
    tokio::signal::ctrl_c().await?;
    let _ = conn.disconnect().await;
    Ok(0)
}

pub async fn serial(args: SerialArgs) -> anyhow::Result<i32> {
    if args.device == "list" {
        for port in friki_serial::list_ports()? {
            println!("{}", sanitize_remote(&port));
        }
        return Ok(0);
    }
    let cfg = SessionConfig {
        name: args.device.clone(),
        protocol: Protocol::Serial,
        serial: SerialConfig {
            device: args.device,
            baud: args.baud,
            data_bits: args.data_bits,
            parity: args.parity,
            stop_bits: args.stop_bits,
            flow: args.flow,
        },
        ..Default::default()
    };
    friki_tui::run(cfg).await
}

pub async fn telnet(args: TelnetArgs) -> anyhow::Result<i32> {
    let usage = || UsageError(format!("destino no válido: {}", args.host));
    let (host, port) = split_host_port(&args.host).ok_or_else(usage)?;
    let cfg = SessionConfig {
        name: host.clone(),
        host,
        port: port.unwrap_or(if args.tls { 992 } else { 23 }),
        protocol: if args.tls {
            Protocol::TelnetTls
        } else {
            Protocol::Telnet
        },
        ..Default::default()
    };
    friki_tui::run(cfg).await
}

pub async fn shell(args: ShellArgs) -> anyhow::Result<i32> {
    let command = (!args.command.is_empty()).then(|| args.command.join(" "));
    let cfg = SessionConfig {
        name: "local".into(),
        protocol: Protocol::LocalShell,
        local_command: command,
        ..Default::default()
    };
    friki_tui::run(cfg).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_y_puerto() {
        assert_eq!(split_host_port("h"), Some(("h".into(), None)));
        assert_eq!(split_host_port("h:23"), Some(("h".into(), Some(23))));
        assert_eq!(split_host_port("[::1]:23"), Some(("::1".into(), Some(23))));
        assert_eq!(split_host_port("[::1]"), Some(("::1".into(), None)));
        assert_eq!(split_host_port("::1"), Some(("::1".into(), None)));
        assert_eq!(split_host_port("h:x"), None);
        assert_eq!(split_host_port("h:0"), None);
        assert_eq!(split_host_port(""), None);
    }

    #[test]
    fn describe_ipv6() {
        let f = Forward::Local {
            bind: String::new(),
            port: 8080,
            host: "::1".into(),
            host_port: 80,
        };
        assert_eq!(describe(&f), "-L 127.0.0.1:8080:[::1]:80");
    }
}
