//! Shell local en un pseudoterminal (TODO.md §4).

use friki_core::transport::Stream;

/// Lanza `command` (o `$SHELL` / `%COMSPEC%` si es `None`) en un PTY con `TERM=term`.
/// El fin del proceso produce `StreamEvent::Exit(código)`.
pub async fn spawn(
    command: Option<&str>,
    term: &str,
    cols: u16,
    rows: u16,
) -> std::io::Result<Stream> {
    let _ = (command, term, cols, rows);
    todo!("friki-pty: spawn")
}
