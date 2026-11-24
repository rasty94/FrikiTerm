//! Interfaz de terminal (ratatui + crossterm): una pestaña por sesión en el MVP.
//! Implementa `HostKeyVerifier` y `AuthPrompter` con diálogos propios.
//!
//! Comunicación diálogos <-> bucle: mientras `Connection::connect` está en curso, el
//! verificador y el prompter (que viven dentro de la conexión) envían por un canal
//! `mpsc` una petición `(Dialog, oneshot::Sender<Answer>)`. El bucle de conexión,
//! que sigue dibujando y leyendo teclado, muestra el primer diálogo de la cola, le
//! pasa las teclas y contesta por el `oneshot`. Si se aborta, los `oneshot` se
//! sueltan y quien espera recibe `None`/`Reject`.

mod dialog;
mod input;
mod render;

use std::collections::VecDeque;
use std::io::{Stdout, stdout};
use std::path::{Path, PathBuf};
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
use friki_core::known_hosts::{HostKey, HostKeyStatus, KnownHosts};
use friki_ssh::{AuthPrompter, Connection, HostKeyDecision, HostKeyVerifier, ShellEvent};
use friki_term::{Key, SessionLogger, Terminal as Emulator};
use futures_util::StreamExt;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use tokio::sync::{mpsc, oneshot};

use dialog::{Answer, Dialog};
use input::{Prefix, PrefixOut, translate_key};
use render::{StatusInfo, draw};

type Request = (Dialog, oneshot::Sender<Answer>);
type Tui = Terminal<CrosstermBackend<Stdout>>;

const SCROLLBACK: usize = 10_000;
const WHEEL_LINES: i32 = 3;

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

struct UiVerifier {
    ui: UiChannel,
    path: PathBuf,
}

// ponytail: KnownHostsVerifier<F> no tiene impl ni firma de `ask` en el contrato;
// se consulta `KnownHosts` directamente. Ver CONTRACT-CHANGES.md.
#[async_trait::async_trait]
impl HostKeyVerifier for UiVerifier {
    async fn verify(&self, host: &str, port: u16, key: &HostKey) -> HostKeyDecision {
        let mut known = match KnownHosts::load(&self.path) {
            Ok(k) => k,
            Err(_) => return HostKeyDecision::Reject,
        };
        let fp = key.fingerprint_sha256();
        let dialog = match known.check(host, port, key) {
            HostKeyStatus::Known => return HostKeyDecision::AcceptOnce,
            HostKeyStatus::Revoked => return HostKeyDecision::Reject,
            HostKeyStatus::Unknown => Dialog::unknown_host(host, port, &key.algorithm, &fp),
            HostKeyStatus::Changed { previous } => Dialog::changed_host(
                host,
                port,
                &key.algorithm,
                &fp,
                &previous.fingerprint_sha256(),
            ),
        };
        match self.ui.ask(dialog).await {
            Some(Answer::HostKey(HostKeyDecision::AcceptAndSave)) => {
                // Se guarda aquí; se devuelve AcceptOnce para que la capa SSH no duplique la entrada.
                match known.add(host, port, key) {
                    Ok(()) => HostKeyDecision::AcceptOnce,
                    Err(_) => HostKeyDecision::Reject,
                }
            }
            Some(Answer::HostKey(d)) => d,
            _ => HostKeyDecision::Reject,
        }
    }
}

struct UiPrompter(UiChannel);

#[async_trait::async_trait]
impl AuthPrompter for UiPrompter {
    async fn password(&self, user: &str, host: &str) -> Option<String> {
        match self.0.ask(Dialog::password(user, host)).await? {
            Answer::Secret(s) => s,
            _ => None,
        }
    }

    async fn passphrase(&self, key_path: &Path) -> Option<String> {
        match self
            .0
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
    ) -> Option<Vec<String>> {
        match self
            .0
            .ask(Dialog::keyboard_interactive(name, instructions, prompts))
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
    let mut tui = Terminal::new(CrosstermBackend::new(stdout()))?;
    run_inner(&mut tui, &session).await
}

fn host_label(s: &SessionConfig) -> String {
    format!("{}:{}", s.host, s.port)
}

async fn run_inner(tui: &mut Tui, session: &SessionConfig) -> anyhow::Result<i32> {
    let host = host_label(session);
    let mut events = EventStream::new();

    // Fase 1: conexión, con diálogos.
    let (req_tx, mut req_rx) = mpsc::unbounded_channel::<Request>();
    let ui = UiChannel(req_tx);
    let verifier = Arc::new(UiVerifier {
        ui: ui.clone(),
        path: friki_core::paths::known_hosts_file(),
    });
    let prompter = Arc::new(UiPrompter(ui));
    let connect = Connection::connect(session, verifier, prompter);
    tokio::pin!(connect);
    let mut queue: VecDeque<Request> = VecDeque::new();
    let info = StatusInfo {
        session: &session.name,
        host: &host,
        state: "conectando...",
    };
    let conn = loop {
        tui.draw(|f| draw(f, None, &info, queue.front().map(|(d, _)| d), false))?;
        tokio::select! {
            res = &mut connect => break res?,
            Some(req) = req_rx.recv() => queue.push_back(req),
            ev = events.next() => match ev {
                Some(Ok(Event::Key(k))) => {
                    if let Some((d, _)) = queue.front_mut() {
                        if let Some(ans) = d.handle_key(&k) {
                            if let Some((_, reply)) = queue.pop_front() {
                                let _ = reply.send(ans);
                            }
                        }
                    } else if k.modifiers.contains(KeyModifiers::CONTROL) && k.code == KeyCode::Char('c') {
                        return Ok(130); // cancelado por el usuario; connect se cancela al soltarse
                    }
                }
                Some(Err(e)) => return Err(e.into()),
                None => return Ok(1),
                _ => {}
            },
        }
    };
    drop(req_rx);

    // Fase 2: shell interactivo.
    let size = tui.size()?;
    let (cols, rows) = (size.width, size.height.saturating_sub(1).max(1));
    let (writer, mut reader) = conn.open_shell(&session.term, cols, rows).await?;
    let mut emu = Emulator::new(rows, cols, SCROLLBACK);
    let mut logger = session.log.as_ref().and_then(|cfg| {
        SessionLogger::open(
            cfg,
            &friki_core::paths::logs_dir(),
            &session.name,
            &session.host,
        )
        .ok()
    });
    let log_input = session.log.as_ref().is_some_and(|c| c.log_input);

    let mut prefix = Prefix::default();
    let mut help = false;
    let mut scrolled: i32 = 0;
    let mut dirty = true;
    let mut exit: Option<i32> = None;
    let mut tick = tokio::time::interval(Duration::from_millis(16));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            ev = events.next() => {
                let Some(ev) = ev else { break };
                match ev? {
                    Event::Key(k) => {
                        if help {
                            help = false;
                            dirty = true;
                            continue;
                        }
                        let out = prefix.feed(&k);
                        dirty = true; // la barra puede cambiar
                        match out {
                            PrefixOut::Quit => break,
                            PrefixOut::ScrollUp | PrefixOut::ScrollDown => {
                                let page = i32::from(rows.saturating_sub(1).max(1));
                                scroll(&mut emu, &mut scrolled, if out == PrefixOut::ScrollUp { page } else { -page });
                            }
                            PrefixOut::Help => help = true,
                            PrefixOut::Armed | PrefixOut::Cancel => {}
                            PrefixOut::Forward | PrefixOut::Literal => {
                                // Literal: Ctrl+] real al remoto.
                                let (key, mods) = if out == PrefixOut::Literal {
                                    (Key::Char(']'), friki_term::Modifiers { ctrl: true, ..Default::default() })
                                } else if let Some(t) = translate_key(&k) {
                                    t
                                } else {
                                    continue;
                                };
                                let bytes = emu.encode_key(key, mods);
                                send(&writer, &mut logger, log_input, &bytes).await?;
                                if scrolled != 0 {
                                    emu.scroll_view(0);
                                    scrolled = 0;
                                }
                            }
                        }
                    }
                    Event::Paste(text) => {
                        let bytes = emu.encode_paste(&text);
                        send(&writer, &mut logger, log_input, &bytes).await?;
                        dirty = true;
                    }
                    Event::Resize(w, h) => {
                        let r = h.saturating_sub(1).max(1);
                        emu.resize(r, w);
                        writer.resize(w, r).await?;
                        dirty = true;
                    }
                    Event::Mouse(m) => {
                        match m.kind {
                            MouseEventKind::ScrollUp => scroll(&mut emu, &mut scrolled, WHEEL_LINES),
                            MouseEventKind::ScrollDown => scroll(&mut emu, &mut scrolled, -WHEEL_LINES),
                            _ => {}
                        }
                        dirty = true;
                    }
                    _ => {}
                }
            }
            se = reader.next_event() => match se {
                Some(ShellEvent::Data(d)) => {
                    if let Some(l) = logger.as_mut() {
                        if l.log_output(&d).is_err() {
                            logger = None; // disco lleno, etc.: la sesión sigue sin registro
                        }
                    }
                    emu.process(&d);
                    dirty = true;
                }
                Some(ShellEvent::ExitStatus(c)) => exit = Some(i32::try_from(c).unwrap_or(1)),
                Some(ShellEvent::Closed) | None => break,
            },
            _ = tick.tick() => {
                if dirty {
                    dirty = false;
                    let screen = emu.screen();
                    let state = if scrolled > 0 { "scrollback" } else if prefix.is_armed() { "Ctrl+] ..." } else { "conectado" };
                    let info = StatusInfo { session: &session.name, host: &host, state };
                    tui.draw(|f| draw(f, Some(&screen), &info, None, help))?;
                }
            }
        }
    }
    let _ = conn.disconnect().await;
    Ok(exit.unwrap_or(0))
}

fn scroll(emu: &mut Emulator, scrolled: &mut i32, delta: i32) {
    *scrolled = (*scrolled + delta).max(0);
    if *scrolled == 0 {
        emu.scroll_view(0);
    } else {
        emu.scroll_view(delta);
    }
}

async fn send(
    writer: &friki_ssh::ShellWriter,
    logger: &mut Option<SessionLogger>,
    log_input: bool,
    bytes: &[u8],
) -> anyhow::Result<()> {
    if bytes.is_empty() {
        return Ok(());
    }
    if log_input {
        if let Some(l) = logger.as_mut() {
            let _ = l.log_input(bytes);
        }
    }
    writer.write(bytes).await?;
    Ok(())
}
