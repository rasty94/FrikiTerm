//! Flujo de terminal común a todos los protocolos (SSH, Telnet, RAW, serie, PTY local).
//! Cada crate de transporte devuelve un `Stream`; la TUI no distingue el origen.

use std::io;
use std::sync::Arc;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamEvent {
    Data(Vec<u8>),
    /// El otro extremo terminó con este código (proceso local, shell remoto…).
    Exit(i32),
    /// Cierre limpio sin código.
    Closed,
    /// Pérdida de la conexión o del dispositivo.
    Lost(String),
}

#[async_trait::async_trait]
pub trait StreamWriter: Send + Sync {
    /// No debe bloquearse esperando a que alguien lea (encolar o fallar).
    async fn write(&self, data: &[u8]) -> io::Result<()>;
    /// Tamaño de la ventana. Los transportes que no lo admiten devuelven `Ok(())`.
    async fn resize(&self, cols: u16, rows: u16) -> io::Result<()>;
}

#[async_trait::async_trait]
pub trait StreamReader: Send {
    /// `None` cuando ya no habrá más eventos.
    async fn next_event(&mut self) -> Option<StreamEvent>;
}

pub type Stream = (Arc<dyn StreamWriter>, Box<dyn StreamReader>);
