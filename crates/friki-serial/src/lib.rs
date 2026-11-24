//! Sesiones por puerto serie (TODO.md §4).

use friki_core::config::SerialConfig;
use friki_core::transport::Stream;

/// Puertos serie disponibles (nombre de dispositivo), ordenados.
pub fn list_ports() -> std::io::Result<Vec<String>> {
    todo!("friki-serial: list_ports")
}

/// Abre el puerto con la configuración dada. `resize` no hace nada. Desconectar el
/// dispositivo produce `StreamEvent::Lost`.
pub async fn open(cfg: &SerialConfig) -> std::io::Result<Stream> {
    let _ = cfg;
    todo!("friki-serial: open")
}
