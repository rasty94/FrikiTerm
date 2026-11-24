//! Qué es cada sesión según su protocolo: etiquetas, `TERM`, opciones de Telnet y texto de
//! las redirecciones. Lógica pura.

use std::net::SocketAddr;

use friki_core::config::{Forward, Protocol, SessionConfig, TerminalProfile};
use friki_telnet::TelnetOptions;
use friki_term::Terminal as Emulator;

/// Etiqueta corta del protocolo para el selector y la barra.
pub fn tag(p: Protocol) -> &'static str {
    match p {
        Protocol::Ssh => "ssh",
        Protocol::Telnet => "telnet",
        Protocol::TelnetTls => "telnets",
        Protocol::Raw => "raw",
        Protocol::Serial => "serie",
        Protocol::LocalShell => "local",
    }
}

/// Destino legible: `host:puerto`, dispositivo y velocidad, u orden local.
pub fn label(s: &SessionConfig) -> String {
    match s.protocol {
        Protocol::Ssh => format!("{}:{}", s.host, s.port),
        Protocol::Telnet | Protocol::TelnetTls | Protocol::Raw => {
            format!("{} {}:{}", tag(s.protocol), s.host, s.port)
        }
        Protocol::Serial => format!("serie {}@{}", s.serial.device, s.serial.baud),
        Protocol::LocalShell => s
            .local_command
            .as_deref()
            .map_or_else(|| "shell local".into(), |c| format!("local: {c}")),
    }
}

/// `TERM` que se anuncia. Regla: con el perfil de terminal por defecto manda `session.term`
/// (compatibilidad con las sesiones existentes); si el perfil se ha personalizado, manda la
/// emulación del perfil (`Terminal::term_name`) y `session.term` se ignora.
pub fn term_for(s: &SessionConfig) -> String {
    if s.terminal == TerminalProfile::default() {
        s.term.clone()
    } else {
        Emulator::term_name(s.terminal.emulation).to_string()
    }
}

/// Opciones de `friki_telnet::connect` para Telnet, Telnet/TLS y RAW.
pub fn telnet_options(s: &SessionConfig, term: &str) -> TelnetOptions {
    TelnetOptions {
        tls: s.protocol == Protocol::TelnetTls,
        raw: s.protocol == Protocol::Raw,
        term: term.to_string(),
        connect_timeout_secs: s.connect_timeout_secs,
    }
}

/// Texto de una redirección; con `listen` (dirección real) sustituye a la de escucha local.
pub fn forward_label(f: &Forward, listen: Option<SocketAddr>) -> String {
    match f {
        Forward::Local {
            bind,
            port,
            host,
            host_port,
        } => format!(
            "L {}>{host}:{host_port}",
            listen.map_or_else(|| format!("{bind}:{port}"), |a| a.to_string())
        ),
        Forward::Remote {
            bind,
            port,
            host,
            host_port,
        } => format!("R {bind}:{port}>{host}:{host_port}"),
        Forward::Dynamic { bind, port } => format!(
            "D {} (SOCKS5)",
            listen.map_or_else(|| format!("{bind}:{port}"), |a| a.to_string())
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use friki_core::config::Emulation;

    fn s(p: Protocol) -> SessionConfig {
        SessionConfig {
            protocol: p,
            host: "h".into(),
            port: 23,
            ..Default::default()
        }
    }

    #[test]
    fn telnet_options_follow_protocol() {
        let o = |p| telnet_options(&s(p), "vt100");
        assert!(!o(Protocol::Telnet).tls && !o(Protocol::Telnet).raw);
        assert!(o(Protocol::TelnetTls).tls && !o(Protocol::TelnetTls).raw);
        assert!(!o(Protocol::Raw).tls && o(Protocol::Raw).raw);
        assert_eq!(o(Protocol::Telnet).term, "vt100");
        assert_eq!(o(Protocol::Raw).connect_timeout_secs, 15);
    }

    #[test]
    fn labels_and_tags_per_protocol() {
        assert_eq!(label(&s(Protocol::Ssh)), "h:23");
        assert_eq!(label(&s(Protocol::TelnetTls)), "telnets h:23");
        let mut ser = s(Protocol::Serial);
        ser.serial.device = "/dev/ttyUSB0".into();
        assert_eq!(label(&ser), "serie /dev/ttyUSB0@115200");
        let mut loc = s(Protocol::LocalShell);
        assert_eq!(label(&loc), "shell local");
        loc.local_command = Some("top".into());
        assert_eq!(label(&loc), "local: top");
        assert_eq!(tag(Protocol::Raw), "raw");
    }

    #[test]
    fn term_rule_default_profile_keeps_session_term() {
        let mut x = s(Protocol::Ssh);
        x.term = "screen".into();
        assert_eq!(term_for(&x), "screen");
        x.terminal.emulation = Emulation::Vt100;
        assert_eq!(term_for(&x), "vt100");
        x.terminal.emulation = Emulation::Xterm;
        x.terminal.theme = "light".into();
        assert_eq!(term_for(&x), "xterm-256color");
    }

    #[test]
    fn forward_labels() {
        let l = Forward::Local {
            bind: "127.0.0.1".into(),
            port: 0,
            host: "db".into(),
            host_port: 5432,
        };
        assert_eq!(forward_label(&l, None), "L 127.0.0.1:0>db:5432");
        let real = "127.0.0.1:4000".parse().ok();
        assert_eq!(forward_label(&l, real), "L 127.0.0.1:4000>db:5432");
        let d = Forward::Dynamic {
            bind: "127.0.0.1".into(),
            port: 1080,
        };
        assert_eq!(forward_label(&d, None), "D 127.0.0.1:1080 (SOCKS5)");
    }
}
