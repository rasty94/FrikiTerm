//! Interfaz de terminal (ratatui + crossterm): una pestaña por sesión en el MVP.
//! Implementa `HostKeyVerifier` y `AuthPrompter` con diálogos propios.

use friki_core::config::SessionConfig;

/// Abre la sesión, la dibuja y devuelve el código de salida del shell remoto.
/// Los errores de `friki_ssh::Error` deben llegar intactos (downcast) a la CLI.
pub async fn run(session: SessionConfig) -> anyhow::Result<i32> {
    let _ = session;
    todo!("friki-tui: run")
}
