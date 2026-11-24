//! Telnet (RFC 854 y opciones NAWS 1073, TTYPE 1091, SGA 858, ECHO 857, LFLOW 1372),
//! Telnet sobre TLS y TCP en crudo (TODO.md §4 y §9).

use friki_core::transport::Stream;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TelnetOptions {
    /// TLS con verificación de certificado y nombre de host obligatoria (rustls).
    pub tls: bool,
    /// Sin negociación Telnet: bytes tal cual (sesión RAW).
    pub raw: bool,
    /// Tipo de terminal anunciado por TTYPE.
    pub term: String,
    pub connect_timeout_secs: u64,
}

/// Conecta y devuelve el flujo. `resize` envía NAWS si el servidor lo aceptó.
pub async fn connect(
    host: &str,
    port: u16,
    opts: &TelnetOptions,
    cols: u16,
    rows: u16,
) -> std::io::Result<Stream> {
    let _ = (host, port, opts, cols, rows);
    todo!("friki-telnet: connect")
}
