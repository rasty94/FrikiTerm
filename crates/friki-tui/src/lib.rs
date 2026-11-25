//! Multiplexor de terminal (ratatui + crossterm): varias sesiones (SSH, Telnet, RAW,
//! serie, shell local) a la vez, en pestañas y paneles divididos.
//!
//! Arquitectura: cada panel (`app::Pane`) es una sesión; una tarea por panel conecta,
//! abre el flujo y reenvía `(PaneId, StreamEvent)` y las peticiones de diálogo
//! (clave de host, autenticación) a un único canal `mpsc`. `app::App` guarda las
//! pestañas (árbol de disposición de `layout`, un panel con foco) y un solo bucle
//! `select!` atiende teclado, mensajes de los paneles y el tick de dibujo.
//!
//! Todo es genérico sobre el backend de ratatui y la fuente de eventos (`run_with`),
//! de modo que se prueba con `TestBackend` y teclas sintéticas; `run`/`run_app` lo
//! unen con crossterm.

mod app;
mod copy_mode;
mod dialog;
mod input;
mod keys;
mod layout;
mod picker;
mod render;
mod script_host;
mod search;
mod theme;
pub use friki_session::{forward_label, label, tag, telnet_options, term_for};
#[cfg(feature = "tn3270")]
mod tn3270;
mod xfer;

use std::io::{Stdout, stdout};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crossterm::event::{
    DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture, Event,
    EventStream,
};
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use friki_core::config::SessionConfig;
use friki_core::transport::{StreamEvent, StreamWriter};
use friki_ssh::{Error, Negotiated};
use friki_term::SessionLogger;
use futures_util::Stream;
use ratatui::Terminal;
use ratatui::backend::{Backend, CrosstermBackend};

use dialog::clean;

type Tui = Terminal<CrosstermBackend<Stdout>>;

const WHEEL_LINES: i32 = 3;

/// Rutas de datos; `run` usa las de `friki_core::paths`, las pruebas las suyas.
pub struct Paths {
    pub known_hosts: PathBuf,
    pub logs_dir: PathBuf,
    /// Sesiones guardadas que ofrece el selector (`*.toml`).
    pub sessions_dir: PathBuf,
    /// Destino de las recepciones Zmodem.
    pub downloads_dir: PathBuf,
    /// Scripts `.rhai`: el selector los ofrece y es el único sitio que pueden leer/escribir.
    pub scripts_dir: PathBuf,
}

impl Paths {
    /// Las rutas por defecto de la aplicación.
    pub fn default_paths() -> Self {
        Self {
            known_hosts: friki_core::paths::known_hosts_file(),
            logs_dir: friki_core::paths::logs_dir(),
            sessions_dir: friki_core::paths::sessions_dir(),
            downloads_dir: directories::UserDirs::new()
                .and_then(|d| d.download_dir().map(Path::to_path_buf))
                .unwrap_or_else(|| friki_core::paths::config_dir().join("downloads")),
            scripts_dir: friki_core::paths::config_dir().join("scripts"),
        }
    }
}

/// Modo raw + pantalla alternativa; se restaura siempre al soltar el guard.
struct TermGuard;

fn restore() {
    let _ = crossterm::execute!(
        stdout(),
        DisableBracketedPaste,
        DisableMouseCapture,
        LeaveAlternateScreen,
        crossterm::cursor::Show
    );
    let _ = disable_raw_mode();
}

impl TermGuard {
    fn enter() -> std::io::Result<Self> {
        let prev = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            restore();
            prev(info);
        }));
        let guard = TermGuard; // si falla algo a medias, el Drop restaura
        enable_raw_mode()?;
        crossterm::execute!(
            stdout(),
            EnterAlternateScreen,
            EnableMouseCapture,
            EnableBracketedPaste
        )?;
        Ok(guard)
    }
}

impl Drop for TermGuard {
    fn drop(&mut self) {
        restore();
    }
}

/// Abre una sola sesión y devuelve el código de salida del shell remoto. Cuando el
/// remoto termina, la aplicación sale sin esperar tecla (compatibilidad con
/// `friki ssh host`). Los `friki_ssh::Error` llegan intactos (downcast) a la CLI.
pub async fn run(session: SessionConfig) -> anyhow::Result<i32> {
    run_term(Some(session), Paths::default_paths(), true).await
}

/// Multiplexor completo. Sin sesión inicial abre el selector de sesiones. Un panel
/// terminado espera una tecla para cerrarse; sale al cerrar la última pestaña o con `quit`.
pub async fn run_app(initial: Option<SessionConfig>, paths: Paths) -> anyhow::Result<i32> {
    run_term(initial, paths, false).await
}

async fn run_term(
    initial: Option<SessionConfig>,
    paths: Paths,
    exit_with_last: bool,
) -> anyhow::Result<i32> {
    let _guard = TermGuard::enter()?;
    let mut tui: Tui = Terminal::new(CrosstermBackend::new(stdout()))?;
    run_with_host(
        &mut tui,
        EventStream::new(),
        initial,
        &paths,
        exit_with_last,
        Box::new(stdout()),
    )
    .await
}

/// Bucle completo sobre cualquier backend y fuente de eventos. Con `exit_with_last`, el
/// fin del único panel abierto termina la aplicación con su resultado, sin tecla.
pub async fn run_with<B, S>(
    tui: &mut Terminal<B>,
    events: S,
    initial: Option<SessionConfig>,
    paths: &Paths,
    exit_with_last: bool,
) -> anyhow::Result<i32>
where
    B: Backend,
    B::Error: std::error::Error + Send + Sync + 'static,
    S: Stream<Item = std::io::Result<Event>> + Unpin,
{
    run_with_host(
        tui,
        events,
        initial,
        paths,
        exit_with_last,
        Box::new(std::io::sink()),
    )
    .await
}

/// Como `run_with`, con la salida de la terminal anfitriona (`host`): ahí se escriben las
/// secuencias OSC 52 del modo copia. `run` usa la salida estándar.
pub async fn run_with_host<B, S>(
    tui: &mut Terminal<B>,
    events: S,
    initial: Option<SessionConfig>,
    paths: &Paths,
    exit_with_last: bool,
    host: Box<dyn std::io::Write + Send>,
) -> anyhow::Result<i32>
where
    B: Backend,
    B::Error: std::error::Error + Send + Sync + 'static,
    S: Stream<Item = std::io::Result<Event>> + Unpin,
{
    app::run_loop(tui, events, initial, paths, exit_with_last, host).await
}

/// Registro de sesión: si no se puede abrir o escribir, queda constancia en `warn`
/// (la barra de estado lo muestra) en vez de desactivarse en silencio.
struct Log {
    logger: Option<SessionLogger>,
    input: bool,
    warn: Option<String>,
}

impl Log {
    fn open(session: &SessionConfig, dir: &Path, warnings: &[String]) -> Self {
        let Some(cfg) = session.log.as_ref() else {
            return Self {
                logger: None,
                input: false,
                warn: None,
            };
        };
        let mut log = Self {
            logger: None,
            input: cfg.log_input,
            warn: None,
        };
        match SessionLogger::open(cfg, dir, &session.name, &session.host) {
            Ok(l) => {
                log.logger = Some(l);
                for w in warnings {
                    log.output(format!("[friki] aviso: {}\r\n", clean(w)).as_bytes());
                }
            }
            Err(e) => log.warn = Some(format!("SIN REGISTRO: {}", clean(&e.to_string()))),
        }
        log
    }

    fn output(&mut self, bytes: &[u8]) {
        if let Some(l) = self.logger.as_mut()
            && let Err(e) = l.log_output(bytes)
        {
            self.logger = None;
            self.warn = Some(format!("REGISTRO DETENIDO: {}", clean(&e.to_string())));
        }
    }

    fn input(&mut self, bytes: &[u8]) {
        if !self.input {
            return;
        }
        if let Some(l) = self.logger.as_mut()
            && let Err(e) = l.log_input(bytes)
        {
            self.logger = None;
            self.warn = Some(format!("REGISTRO DETENIDO: {}", clean(&e.to_string())));
        }
    }
}

/// Escritura al otro extremo. Un canal ya cerrado (`BrokenPipe`: carrera al hacer `exit`)
/// no es un error de la sesión: el evento final llegará por el lector. Cualquier otro fallo
/// (cola llena...) descarta la entrada y lo anota para la barra de estado.
struct Out {
    writer: Arc<dyn StreamWriter>,
    log: Log,
    dropped: bool,
}

impl Out {
    fn settle(&mut self, r: std::io::Result<()>) {
        match r {
            Ok(()) => self.dropped = false,
            Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => {}
            Err(_) => self.dropped = true,
        }
    }

    async fn send(&mut self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        self.log.input(bytes);
        let r = self.writer.write(bytes).await;
        self.settle(r);
    }

    async fn resize(&mut self, cols: u16, rows: u16) {
        let r = self.writer.resize(cols, rows).await;
        self.settle(r);
    }
}

/// Código de salida que aporta un evento del flujo, si es de salida.
fn exit_code(ev: &StreamEvent) -> Option<i32> {
    match ev {
        StreamEvent::Exit(c) => Some(*c),
        _ => None,
    }
}

/// Resultado final cuando el lector termina (`Closed`, `Lost` o canal agotado).
/// `Lost` se devuelve como `friki_ssh::Error::Network` (error de red genérico en todos los
/// protocolos), sin envolver en texto.
fn finish(exit: Option<i32>, last: Option<StreamEvent>) -> anyhow::Result<i32> {
    match last {
        Some(StreamEvent::Lost(msg)) => Err(Error::Network(msg).into()),
        Some(StreamEvent::Closed) => Ok(exit.unwrap_or(0)),
        _ => exit.ok_or_else(|| Error::Network("canal cerrado sin estado de salida".into()).into()),
    }
}

/// Cifrado sin el sufijo de dominio (`@openssh.com`).
fn compact(name: &str) -> String {
    clean(name.split('@').next().unwrap_or(name))
}

/// Algoritmos que la política segura nunca ofrece.
fn is_legacy(n: &Negotiated) -> bool {
    !n.weak().is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exit_events_map_to_codes() {
        assert_eq!(exit_code(&StreamEvent::Exit(3)), Some(3));
        assert_eq!(exit_code(&StreamEvent::Exit(137)), Some(137));
        assert_eq!(exit_code(&StreamEvent::Closed), None);
        assert_eq!(exit_code(&StreamEvent::Lost("x".into())), None);
        assert_eq!(exit_code(&StreamEvent::Data(vec![1])), None);
    }

    #[test]
    fn finish_maps_final_events() {
        assert_eq!(finish(Some(7), Some(StreamEvent::Closed)).ok(), Some(7));
        assert_eq!(finish(None, Some(StreamEvent::Closed)).ok(), Some(0));
        let e = finish(Some(1), Some(StreamEvent::Lost("corte".into()))).expect_err("lost");
        assert!(matches!(e.downcast_ref::<Error>(), Some(Error::Network(m)) if m == "corte"));
        let e = finish(None, None).expect_err("sin estado");
        assert!(matches!(e.downcast_ref::<Error>(), Some(Error::Network(_))));
        assert_eq!(finish(Some(2), None).ok(), Some(2));
    }

    #[test]
    fn legacy_detection_and_compact() {
        let n = |cipher: &str, host_key: &str| Negotiated {
            kex: "curve25519-sha256".into(),
            cipher: cipher.into(),
            mac: "hmac-sha2-256".into(),
            host_key: host_key.into(),
        };
        assert!(!is_legacy(&n("aes256-gcm@openssh.com", "ssh-ed25519")));
        assert!(is_legacy(&n("aes128-cbc", "ssh-ed25519")));
        assert!(is_legacy(&n("aes128-ctr", "ssh-rsa")));
        assert_eq!(
            compact("chacha20-poly1305@openssh.com"),
            "chacha20-poly1305"
        );
    }
}
