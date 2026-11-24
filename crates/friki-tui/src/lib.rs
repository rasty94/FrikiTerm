//! Interfaz de terminal (ratatui + crossterm): una pestaña por sesión en el MVP.
//! Usa `KnownHostsVerifier` (el único verificador del proyecto) y un `AuthPrompter`
//! propio, ambos con diálogos.
//!
//! Comunicación diálogos <-> bucle: mientras `Connection::connect` está en curso, el
//! verificador y el prompter (que viven dentro de la conexión) envían por un canal
//! `mpsc` una petición `(Dialog, oneshot::Sender<Answer>)`. El bucle de conexión,
//! que sigue dibujando y leyendo teclado, muestra el primer diálogo de la cola, le
//! pasa las teclas y contesta por el `oneshot`. Si se aborta, los `oneshot` se
//! sueltan y quien espera recibe `None`/`Reject`.
//!
//! Los bucles son genéricos sobre el backend de ratatui y sobre la fuente de eventos
//! (`run_with`, `connect_ui`, `shell_session`), de modo que se pueden probar con
//! `TestBackend` y teclas sintéticas; `run` los une con crossterm.

mod dialog;
mod input;
mod render;

use std::collections::VecDeque;
use std::future::Future;
use std::io::{Stdout, stdout};
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use crossterm::event::{
    DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture, Event,
    EventStream, KeyCode, KeyModifiers, MouseEventKind,
};
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use friki_core::config::SessionConfig;
use friki_core::known_hosts::{HostKey, HostKeyStatus};
use friki_ssh::{
    AuthPrompter, Connection, Error, HostKeyDecision, KnownHostsVerifier, Negotiated, ShellEvent,
    ShellWriter, Zeroizing, exit_code_for_signal,
};
use friki_term::{Key, Modifiers, SessionLogger, Terminal as Emulator};
use futures_util::{Stream, StreamExt};
use ratatui::Terminal;
use ratatui::backend::{Backend, CrosstermBackend};
use tokio::sync::{mpsc, oneshot};

use dialog::{Answer, Dialog, clean};
use input::{Prefix, PrefixOut, pressed, translate_key};
use render::{StatusInfo, draw};

type Request = (Dialog, oneshot::Sender<Answer>);
type Tui = Terminal<CrosstermBackend<Stdout>>;

const SCROLLBACK: usize = 10_000;
const WHEEL_LINES: i32 = 3;

/// Rutas de datos; `run` usa las de `friki_core::paths`, las pruebas las suyas.
pub struct Paths {
    pub known_hosts: PathBuf,
    pub logs_dir: PathBuf,
}

/// Envía diálogos al bucle principal y espera la respuesta.
#[derive(Clone)]
struct UiChannel(mpsc::UnboundedSender<Request>);

impl UiChannel {
    async fn ask(&self, d: Dialog) -> Option<Answer> {
        let (tx, rx) = oneshot::channel();
        self.0.send((d, tx)).ok()?;
        rx.await.ok()
    }
}

struct UiPrompter {
    ui: UiChannel,
    host: String,
}

#[async_trait::async_trait]
impl AuthPrompter for UiPrompter {
    async fn password(&self, user: &str, host: &str) -> Option<Zeroizing<String>> {
        match self.ui.ask(Dialog::password(user, host)).await? {
            Answer::Secret(s) => s,
            _ => None,
        }
    }

    async fn passphrase(&self, key_path: &Path) -> Option<Zeroizing<String>> {
        match self
            .ui
            .ask(Dialog::passphrase(&key_path.display().to_string()))
            .await?
        {
            Answer::Secret(s) => s,
            _ => None,
        }
    }

    async fn keyboard_interactive(
        &self,
        name: &str,
        instructions: &str,
        prompts: &[(String, bool)],
    ) -> Option<Vec<Zeroizing<String>>> {
        match self
            .ui
            .ask(Dialog::keyboard_interactive(
                &self.host,
                name,
                instructions,
                prompts,
            ))
            .await?
        {
            Answer::Multi(m) => m,
            _ => None,
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

/// Abre la sesión, la dibuja y devuelve el código de salida del shell remoto.
/// Los errores de `friki_ssh::Error` deben llegar intactos (downcast) a la CLI.
pub async fn run(session: SessionConfig) -> anyhow::Result<i32> {
    let _guard = TermGuard::enter()?;
    let mut tui: Tui = Terminal::new(CrosstermBackend::new(stdout()))?;
    let paths = Paths {
        known_hosts: friki_core::paths::known_hosts_file(),
        logs_dir: friki_core::paths::logs_dir(),
    };
    run_with(&mut tui, EventStream::new(), &session, &paths).await
}

/// Conexión con diálogos y después shell interactivo, sobre cualquier backend y
/// fuente de eventos.
pub async fn run_with<B, S>(
    tui: &mut Terminal<B>,
    mut events: S,
    session: &SessionConfig,
    paths: &Paths,
) -> anyhow::Result<i32>
where
    B: Backend,
    B::Error: std::error::Error + Send + Sync + 'static,
    S: Stream<Item = std::io::Result<Event>> + Unpin,
{
    let conn = match connect_ui(tui, &mut events, session, paths).await? {
        Connected::Ready(c) => c,
        Connected::Cancelled => return Ok(130),
        Connected::InputClosed => return Ok(1),
    };
    shell_session(tui, &mut events, conn, session, paths).await
}

pub enum Connected {
    Ready(Connection),
    /// El usuario pulsó Ctrl+C sin diálogo abierto.
    Cancelled,
    /// Se cerró la fuente de eventos (terminal desaparecida).
    InputClosed,
}

fn host_label(s: &SessionConfig) -> String {
    format!("{}:{}", s.host, s.port)
}

type Ask = Pin<Box<dyn Future<Output = HostKeyDecision> + Send>>;

/// Fase 1: conexión, con diálogos de clave de host y de autenticación.
pub async fn connect_ui<B, S>(
    tui: &mut Terminal<B>,
    events: &mut S,
    session: &SessionConfig,
    paths: &Paths,
) -> anyhow::Result<Connected>
where
    B: Backend,
    B::Error: std::error::Error + Send + Sync + 'static,
    S: Stream<Item = std::io::Result<Event>> + Unpin,
{
    let host = host_label(session);
    let (req_tx, mut req_rx) = mpsc::unbounded_channel::<Request>();
    let ui = UiChannel(req_tx);
    let ask_ui = ui.clone();
    let verifier = Arc::new(KnownHostsVerifier::new(
        paths.known_hosts.clone(),
        move |host: &str, port: u16, key: &HostKey, status: HostKeyStatus| -> Ask {
            let fp = key.fingerprint_sha256();
            let dialog = match status {
                HostKeyStatus::Unknown => Dialog::unknown_host(host, port, &key.algorithm, &fp),
                HostKeyStatus::Changed { previous } => Dialog::changed_host(
                    host,
                    port,
                    &key.algorithm,
                    &fp,
                    &previous.fingerprint_sha256(),
                ),
                _ => return Box::pin(async { HostKeyDecision::Reject }),
            };
            let ui = ask_ui.clone();
            Box::pin(async move {
                match ui.ask(dialog).await {
                    Some(Answer::HostKey(d)) => d,
                    _ => HostKeyDecision::Reject,
                }
            })
        },
    ));
    let prompter = Arc::new(UiPrompter {
        ui,
        host: session.host.clone(),
    });
    let connect = Connection::connect(session, verifier, prompter);
    tokio::pin!(connect);
    let mut queue: VecDeque<Request> = VecDeque::new();
    let info = StatusInfo {
        session: &session.name,
        host: &host,
        state: "conectando...",
        ..Default::default()
    };
    loop {
        tui.draw(|f| draw(f, None, &info, queue.front().map(|(d, _)| d), false))?;
        tokio::select! {
            res = &mut connect => return Ok(Connected::Ready(res?)),
            Some(req) = req_rx.recv() => queue.push_back(req),
            ev = events.next() => match ev {
                Some(Ok(Event::Key(k))) if pressed(&k) => {
                    if let Some((d, _)) = queue.front_mut() {
                        if let Some(ans) = d.handle_key(&k)
                            && let Some((_, reply)) = queue.pop_front()
                        {
                            let _ = reply.send(ans);
                        }
                    } else if k.modifiers.contains(KeyModifiers::CONTROL) && k.code == KeyCode::Char('c') {
                        return Ok(Connected::Cancelled); // connect se cancela al soltarse
                    }
                }
                Some(Ok(Event::Paste(text))) => {
                    if let Some((d, _)) = queue.front_mut() {
                        d.paste(&text);
                    }
                }
                Some(Err(e)) => return Err(e.into()),
                None => return Ok(Connected::InputClosed),
                _ => {}
            },
        }
    }
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

/// Escritura al remoto. Un canal ya cerrado (carrera al hacer `exit`) no es un error
/// de la sesión: el evento final llegará por el lector. Una cola llena descarta la
/// entrada y lo anota para la barra de estado.
struct Out {
    writer: ShellWriter,
    log: Log,
    dropped: bool,
}

impl Out {
    fn settle(&mut self, r: friki_ssh::Result<()>) -> anyhow::Result<()> {
        match r {
            Ok(()) => self.dropped = false,
            Err(Error::Network(_)) => {}
            Err(Error::Other(_)) => self.dropped = true,
            Err(e) => return Err(e.into()),
        }
        Ok(())
    }

    async fn send(&mut self, bytes: &[u8]) -> anyhow::Result<()> {
        if bytes.is_empty() {
            return Ok(());
        }
        self.log.input(bytes);
        let r = self.writer.write(bytes).await;
        self.settle(r)
    }

    async fn resize(&mut self, cols: u16, rows: u16) -> anyhow::Result<()> {
        let r = self.writer.resize(cols, rows).await;
        self.settle(r)
    }
}

/// Código de salida que aporta un evento del shell, si es de estado o señal.
fn exit_code(ev: &ShellEvent) -> Option<i32> {
    match ev {
        ShellEvent::ExitStatus(c) => Some(i32::try_from(*c).unwrap_or(255)),
        ShellEvent::ExitSignal { signal, .. } => Some(exit_code_for_signal(signal)),
        _ => None,
    }
}

/// Resultado final cuando el lector termina (`Closed`, `Lost` o canal agotado).
/// `Lost` se devuelve como `friki_ssh::Error::Network` sin envolver en texto.
fn finish(exit: Option<i32>, last: Option<ShellEvent>) -> anyhow::Result<i32> {
    match last {
        Some(ShellEvent::Lost(msg)) => Err(Error::Network(msg).into()),
        Some(ShellEvent::Closed) => Ok(exit.unwrap_or(0)),
        _ => exit.ok_or_else(|| Error::Network("canal cerrado sin estado de salida".into()).into()),
    }
}

/// Cifrado sin el sufijo de dominio (`@openssh.com`).
fn compact(name: &str) -> String {
    clean(name.split('@').next().unwrap_or(name))
}

/// Algoritmos que la política segura nunca ofrece (equivale a `Negotiated::weak`).
fn is_legacy(n: &Negotiated) -> bool {
    [&n.kex, &n.cipher, &n.mac, &n.host_key]
        .into_iter()
        .any(|a| {
            ["3des", "-cbc", "sha1", "md5", "group1-", "dss"]
                .iter()
                .any(|d| a.contains(d))
                || a == "ssh-rsa"
        })
}

/// Diálogo modal durante la sesión; con `paste`, el texto a enviar si se confirma.
struct Modal {
    dialog: Dialog,
    paste: Option<String>,
}

/// Fase 2: shell interactivo hasta que el remoto termina o el usuario sale.
pub async fn shell_session<B, S>(
    tui: &mut Terminal<B>,
    events: &mut S,
    conn: Connection,
    session: &SessionConfig,
    paths: &Paths,
) -> anyhow::Result<i32>
where
    B: Backend,
    B::Error: std::error::Error + Send + Sync + 'static,
    S: Stream<Item = std::io::Result<Event>> + Unpin,
{
    let host = host_label(session);
    let size = tui.size()?;
    let (cols, rows) = (size.width.max(1), size.height.saturating_sub(1).max(1));
    let (writer, mut reader) = conn.open_shell(&session.term, cols, rows).await?;
    let mut emu = Emulator::new(rows, cols, SCROLLBACK);

    let warnings = conn.warnings();
    let negotiated = conn.negotiated();
    let cipher = negotiated
        .as_ref()
        .map(|n| compact(&n.cipher))
        .unwrap_or_default();
    let legacy = session.legacy_algorithms || negotiated.as_ref().is_some_and(is_legacy);
    let mut out = Out {
        writer,
        log: Log::open(session, &paths.logs_dir, &warnings),
        dropped: false,
    };
    let mut modal = (!warnings.is_empty()).then(|| Modal {
        dialog: Dialog::notice("Avisos de la conexión", &warnings, true),
        paste: None,
    });

    let mut prefix = Prefix::default();
    let mut help = false;
    let mut dirty = true;
    let mut exit: Option<i32> = None;
    let mut user_quit = false;
    let mut last: Option<ShellEvent> = None;
    let mut tick = tokio::time::interval(Duration::from_millis(16));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    let drawn = loop {
        tokio::select! {
            ev = events.next() => {
                let Some(ev) = ev else { break None };
                match ev? {
                    Event::Key(k) if !pressed(&k) => {}
                    Event::Key(k) => {
                        dirty = true;
                        if let Some(m) = modal.as_mut() {
                            if let Some(ans) = m.dialog.handle_key(&k) {
                                let text = m.paste.take();
                                modal = None;
                                if let (Answer::Confirm(true), Some(t)) = (ans, text) {
                                    let bytes = emu.encode_paste(&t);
                                    out.send(&bytes).await?;
                                }
                            }
                            continue;
                        }
                        if help {
                            help = false;
                            continue;
                        }
                        let o = prefix.feed(&k);
                        match o {
                            PrefixOut::Quit => {
                                user_quit = true;
                                break None;
                            }
                            PrefixOut::ScrollUp | PrefixOut::ScrollDown => {
                                let page = i32::from(rows.saturating_sub(1).max(1));
                                scroll(&mut emu, if o == PrefixOut::ScrollUp { page } else { -page });
                            }
                            PrefixOut::Help => help = true,
                            PrefixOut::Armed | PrefixOut::Cancel => {}
                            PrefixOut::Forward | PrefixOut::Literal => {
                                // Literal: Ctrl+] real al remoto.
                                let (key, mods) = if o == PrefixOut::Literal {
                                    (Key::Char(']'), Modifiers { ctrl: true, ..Default::default() })
                                } else if let Some(t) = translate_key(&k) {
                                    t
                                } else {
                                    continue;
                                };
                                let bytes = emu.encode_key(key, mods);
                                out.send(&bytes).await?;
                                if emu.scroll_offset() != 0 {
                                    emu.scroll_view(0);
                                }
                            }
                        }
                    }
                    Event::Paste(text) => {
                        dirty = true;
                        if let Some(m) = modal.as_mut() {
                            m.dialog.paste(&text);
                        } else if emu.paste_needs_confirmation(&text) {
                            let n = text.trim_end_matches(['\n', '\r']).lines().count();
                            modal = Some(Modal {
                                dialog: Dialog::paste_confirm(n),
                                paste: Some(text),
                            });
                        } else {
                            let bytes = emu.encode_paste(&text);
                            out.send(&bytes).await?;
                        }
                    }
                    Event::Resize(w, h) => {
                        let (w, r) = (w.max(1), h.saturating_sub(1).max(1));
                        emu.resize(r, w);
                        out.resize(w, r).await?;
                        dirty = true;
                    }
                    Event::Mouse(m) => {
                        match m.kind {
                            MouseEventKind::ScrollUp => scroll(&mut emu, WHEEL_LINES),
                            MouseEventKind::ScrollDown => scroll(&mut emu, -WHEEL_LINES),
                            _ => {}
                        }
                        dirty = true;
                    }
                    _ => {}
                }
            }
            se = reader.next_event() => match se {
                Some(ShellEvent::Data(d)) => {
                    out.log.output(&d);
                    emu.process(&d);
                    dirty = true;
                }
                Some(ev) => {
                    if let Some(c) = exit_code(&ev) {
                        exit = Some(c); // se sigue drenando hasta Closed/Lost
                    } else {
                        last = Some(ev);
                        break Some(());
                    }
                }
                None => break Some(()),
            },
            _ = tick.tick() => {
                if dirty {
                    dirty = false;
                    draw_session(tui, &emu, &session.name, &host, &StatusExtra {
                        prefix_armed: prefix.is_armed(),
                        cipher: &cipher,
                        legacy,
                        out: &out,
                    }, modal.as_ref().map(|m| &m.dialog), help)?;
                }
            }
        }
    };
    // Último dibujo con lo que llegó justo antes de cerrar.
    draw_session(
        tui,
        &emu,
        &session.name,
        &host,
        &StatusExtra {
            prefix_armed: false,
            cipher: &cipher,
            legacy,
            out: &out,
        },
        None,
        false,
    )?;
    let _ = tokio::time::timeout(Duration::from_secs(2), conn.disconnect()).await;
    if drawn.is_none() {
        // Salida del usuario (o fin de la entrada): sin evento final del remoto.
        return Ok(exit.unwrap_or(if user_quit { 0 } else { 1 }));
    }
    finish(exit, last)
}

struct StatusExtra<'a> {
    prefix_armed: bool,
    cipher: &'a str,
    legacy: bool,
    out: &'a Out,
}

fn draw_session<B>(
    tui: &mut Terminal<B>,
    emu: &Emulator,
    name: &str,
    host: &str,
    x: &StatusExtra,
    dialog: Option<&Dialog>,
    help: bool,
) -> anyhow::Result<()>
where
    B: Backend,
    B::Error: std::error::Error + Send + Sync + 'static,
{
    let screen = emu.screen();
    let state = if x.out.dropped {
        "ENTRADA DESCARTADA (cola llena)".to_string()
    } else if emu.alternate_screen() {
        "pantalla alternativa".to_string()
    } else if emu.scroll_offset() > 0 {
        format!(
            "scrollback {}/{}",
            emu.scroll_offset(),
            emu.scrollback_len()
        )
    } else if x.prefix_armed {
        "Ctrl+] ...".to_string()
    } else {
        "conectado".to_string()
    };
    let info = StatusInfo {
        session: name,
        host,
        state: &state,
        cipher: x.cipher,
        legacy: x.legacy,
        log_warn: x.out.log.warn.as_deref(),
    };
    tui.draw(|f| draw(f, Some(&screen), &info, dialog, help))?;
    Ok(())
}

/// Desplaza la vista; en la pantalla alternativa no hay scrollback y se ignora.
fn scroll(emu: &mut Emulator, delta: i32) {
    if !emu.alternate_screen() {
        emu.scroll_view(delta);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exit_events_map_to_codes() {
        assert_eq!(exit_code(&ShellEvent::ExitStatus(3)), Some(3));
        assert_eq!(
            exit_code(&ShellEvent::ExitSignal {
                signal: "KILL".into(),
                core_dumped: false
            }),
            Some(137)
        );
        assert_eq!(exit_code(&ShellEvent::Closed), None);
        assert_eq!(exit_code(&ShellEvent::Data(vec![1])), None);
    }

    #[test]
    fn finish_maps_final_events() {
        assert_eq!(finish(Some(7), Some(ShellEvent::Closed)).ok(), Some(7));
        assert_eq!(finish(None, Some(ShellEvent::Closed)).ok(), Some(0));
        let e = finish(Some(1), Some(ShellEvent::Lost("corte".into()))).expect_err("lost");
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
