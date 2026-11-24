//! Aplicación: pestañas, paneles (una sesión cada uno) y el bucle único.
//!
//! Cada panel lanza una tarea que conecta, abre el shell y reenvía todo (diálogos de
//! conexión, eventos del shell) por un único canal a este bucle. Los diálogos de clave
//! de host y autenticación se muestran siempre de forma modal, con el nombre de la
//! sesión a la que pertenecen, aunque otro panel tenga el foco.

use std::collections::{BTreeMap, VecDeque};
use std::future::Future;
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use crossterm::event::{Event, KeyEvent, MouseEventKind};
use friki_core::config::{AppConfig, SessionConfig, load_sessions};
use friki_core::known_hosts::{HostKey, HostKeyStatus};
use friki_ssh::{
    AuthPrompter, Connection, Error, HostKeyDecision, KnownHostsVerifier, Negotiated, ShellEvent,
    ShellWriter, Zeroizing,
};
use friki_term::{Key, Modifiers, Terminal as Emulator};
use futures_util::{Stream, StreamExt};
use ratatui::Frame;
use ratatui::Terminal;
use ratatui::backend::Backend;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;

use crate::dialog::{Answer, Dialog, clean};
use crate::input::pressed;
use crate::keys::{Action, KeyOut, Keymap};
use crate::layout::{Dir, Node, PaneId, cycle};
use crate::picker::{Picker, PickerOut};
use crate::render::{StatusInfo, draw_dialog, popup, screen_to_buffer, status_line};
use crate::{Log, Out, Paths, SCROLLBACK, WHEEL_LINES, compact, exit_code, finish, is_legacy};

type Ask = Pin<Box<dyn Future<Output = HostKeyDecision> + Send>>;

/// Lo que una tarea de panel envía al bucle central.
enum Msg {
    Ask(PaneId, Dialog, oneshot::Sender<Answer>),
    Opened(PaneId, Box<Opened>),
    Failed(PaneId, Error),
    Shell(PaneId, ShellEvent),
    ReaderEnd(PaneId),
}

struct Opened {
    conn: Connection,
    writer: ShellWriter,
    /// (columnas, filas) con las que se abrió el PTY.
    size: (u16, u16),
    warnings: Vec<String>,
    negotiated: Option<Negotiated>,
}

/// Envía diálogos al bucle principal y espera la respuesta.
#[derive(Clone)]
struct UiChannel {
    tx: mpsc::UnboundedSender<Msg>,
    pane: PaneId,
}

impl UiChannel {
    async fn ask(&self, d: Dialog) -> Option<Answer> {
        let (tx, rx) = oneshot::channel();
        self.tx.send(Msg::Ask(self.pane, d, tx)).ok()?;
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

/// Conecta, abre el shell y reenvía sus eventos hasta que el canal se agota.
fn spawn_connect(
    tx: mpsc::UnboundedSender<Msg>,
    id: PaneId,
    session: SessionConfig,
    known_hosts: std::path::PathBuf,
    (cols, rows): (u16, u16),
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let ui = UiChannel {
            tx: tx.clone(),
            pane: id,
        };
        let ask_ui = ui.clone();
        let verifier = Arc::new(KnownHostsVerifier::new(
            known_hosts,
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
        let conn = match Connection::connect(&session, verifier, prompter).await {
            Ok(c) => c,
            Err(e) => {
                let _ = tx.send(Msg::Failed(id, e));
                return;
            }
        };
        let (writer, mut reader) = match conn.open_shell(&session.term, cols, rows).await {
            Ok(x) => x,
            Err(e) => {
                let _ = tx.send(Msg::Failed(id, e));
                return;
            }
        };
        let opened = Opened {
            warnings: conn.warnings(),
            negotiated: conn.negotiated(),
            conn,
            writer,
            size: (cols, rows),
        };
        if tx.send(Msg::Opened(id, Box::new(opened))).is_err() {
            return;
        }
        while let Some(ev) = reader.next_event().await {
            if tx.send(Msg::Shell(id, ev)).is_err() {
                return;
            }
        }
        let _ = tx.send(Msg::ReaderEnd(id));
    })
}

/// Diálogo modal de un panel; con `paste`, el texto a enviar si se confirma.
struct Modal {
    dialog: Dialog,
    paste: Option<String>,
}

enum State {
    Connecting,
    Live,
    /// Terminada (con la línea que se muestra); espera una tecla para cerrarse.
    Ended(String),
    Failed(String),
}

struct Pane {
    session: SessionConfig,
    host: String,
    state: State,
    emu: Option<Emulator>,
    out: Option<Out>,
    /// (columnas, filas) del área interior.
    size: (u16, u16),
    cipher: String,
    legacy: bool,
    modal: Option<Modal>,
    conn: Option<Connection>,
    task: Option<JoinHandle<()>>,
    exit: Option<i32>,
    outcome: Option<anyhow::Result<i32>>,
}

impl Pane {
    fn over(&self) -> bool {
        matches!(self.state, State::Ended(_) | State::Failed(_))
    }
}

struct Tab {
    name: String,
    root: Node,
    focus: PaneId,
}

#[derive(Clone, Copy)]
enum Mode {
    Tab,
    Split(Dir),
}

enum Overlay {
    Help,
    Picker { picker: Picker, mode: Mode },
    Rename(Dialog),
}

struct App<'a> {
    tabs: Vec<Tab>,
    cur: usize,
    panes: BTreeMap<PaneId, Pane>,
    next_id: PaneId,
    keymap: Keymap,
    tx: mpsc::UnboundedSender<Msg>,
    /// Diálogos de conexión pendientes (de cualquier panel), en orden de llegada.
    asks: VecDeque<(PaneId, Dialog, oneshot::Sender<Answer>)>,
    overlay: Option<Overlay>,
    paths: &'a Paths,
    exit_with_last: bool,
    opened_total: usize,
    last_outcome: Option<anyhow::Result<i32>>,
    done: Option<anyhow::Result<i32>>,
    size: (u16, u16),
    notice: Option<String>,
    disconnects: Vec<JoinHandle<()>>,
}

fn inner(r: Rect, multi: bool) -> Rect {
    if multi {
        Rect {
            y: r.y + 1,
            height: r.height.saturating_sub(1),
            ..r
        }
    } else {
        r
    }
}

fn compute(tab: &Tab, body: Rect) -> (Vec<(PaneId, Rect)>, Vec<Rect>) {
    let (mut panes, mut seps) = (vec![], vec![]);
    tab.root.areas(body, &mut panes, &mut seps);
    (panes, seps)
}

/// Área de las pestañas: sin la barra de pestañas (si hay más de una) ni la de estado.
fn body_of(w: u16, h: u16, tabs: usize) -> Rect {
    let top = u16::from(tabs > 1);
    Rect::new(0, top, w, h.saturating_sub(top + 1))
}

/// Desplaza la vista; en la pantalla alternativa no hay scrollback y se ignora.
fn scroll(emu: &mut Emulator, delta: i32) {
    if !emu.alternate_screen() {
        emu.scroll_view(delta);
    }
}

fn ended_label(r: &anyhow::Result<i32>) -> String {
    match r {
        Ok(c) => format!("[sesión terminada: código {c}]"),
        Err(e) => format!("[sesión perdida: {}]", clean(&e.to_string())),
    }
}

impl<'a> App<'a> {
    fn new(tx: mpsc::UnboundedSender<Msg>, paths: &'a Paths, exit_with_last: bool) -> Self {
        Self {
            tabs: vec![],
            cur: 0,
            panes: BTreeMap::new(),
            next_id: 1,
            // ponytail: `load_app_config` aún es `todo!()` en esta rama; cargar el fichero
            // (`paths::app_config_file`) cuando esté implementado.
            keymap: Keymap::new(&AppConfig::default()),
            tx,
            asks: VecDeque::new(),
            overlay: None,
            paths,
            exit_with_last,
            opened_total: 0,
            last_outcome: None,
            done: None,
            size: (80, 24),
            notice: None,
            disconnects: vec![],
        }
    }

    fn focus_id(&self) -> Option<PaneId> {
        self.tabs.get(self.cur).map(|t| t.focus)
    }

    fn open_picker(&mut self, mode: Mode) {
        let (sessions, err) = match load_sessions(&self.paths.sessions_dir) {
            Ok(s) => (s, None),
            Err(e) => (
                vec![],
                Some(format!("sesiones guardadas: {}", clean(&e.to_string()))),
            ),
        };
        self.overlay = Some(Overlay::Picker {
            picker: Picker::new(sessions, err),
            mode,
        });
    }

    /// Crea el panel, lo coloca (pestaña nueva o división del panel con foco) y lanza la
    /// conexión en segundo plano.
    async fn open(&mut self, session: SessionConfig, mode: Mode) {
        let id = self.next_id;
        self.next_id += 1;
        self.opened_total += 1;
        let name = clean(&session.name);
        let pane = Pane {
            host: format!("{}:{}", session.host, session.port),
            session: session.clone(),
            state: State::Connecting,
            emu: None,
            out: None,
            size: (80, 24),
            cipher: String::new(),
            legacy: false,
            modal: None,
            conn: None,
            task: None,
            exit: None,
            outcome: None,
        };
        self.panes.insert(id, pane);
        let mut split = false;
        if let (Mode::Split(dir), Some(tab)) = (mode, self.tabs.get_mut(self.cur))
            && tab.root.split(tab.focus, id, dir)
        {
            tab.focus = id;
            split = true;
        }
        if !split {
            self.tabs.push(Tab {
                name,
                root: Node::Leaf(id),
                focus: id,
            });
            self.cur = self.tabs.len() - 1;
        }
        self.relayout().await;
        let size = self.panes.get(&id).map_or((80, 24), |p| p.size);
        let task = spawn_connect(
            self.tx.clone(),
            id,
            session,
            self.paths.known_hosts.clone(),
            size,
        );
        if let Some(p) = self.panes.get_mut(&id) {
            p.task = Some(task);
        }
    }

    /// Recalcula el tamaño interior de cada panel y avisa al emulador y al remoto.
    async fn relayout(&mut self) {
        let body = body_of(self.size.0, self.size.1, self.tabs.len());
        let mut errs = vec![];
        for tab in &self.tabs {
            let (panes, _) = compute(tab, body);
            let multi = panes.len() > 1;
            for (id, r) in panes {
                let i = inner(r, multi);
                let size = (i.width.max(1), i.height.max(1));
                let Some(p) = self.panes.get_mut(&id) else {
                    continue;
                };
                if p.size == size {
                    continue;
                }
                p.size = size;
                if let Some(e) = p.emu.as_mut() {
                    e.resize(size.1, size.0);
                }
                if let Some(o) = p.out.as_mut()
                    && let Err(e) = o.resize(size.0, size.1).await
                {
                    errs.push((id, e));
                }
            }
        }
        for (id, e) in errs {
            self.end_pane(id, Err(e));
        }
    }

    fn detach(&mut self, conn: Connection) {
        self.disconnects.push(tokio::spawn(async move {
            let _ = tokio::time::timeout(Duration::from_secs(2), conn.disconnect()).await;
        }));
    }

    /// El remoto terminó (o falló la escritura): el panel queda con su mensaje a la espera
    /// de una tecla. Con `exit_with_last` y un solo panel, la aplicación sale ya.
    fn end_pane(&mut self, id: PaneId, outcome: anyhow::Result<i32>) {
        let Some(p) = self.panes.get_mut(&id) else {
            return;
        };
        if p.over() {
            return;
        }
        p.state = State::Ended(ended_label(&outcome));
        p.outcome = Some(outcome);
        p.out = None;
        let (task, conn) = (p.task.take(), p.conn.take());
        if let Some(t) = task {
            t.abort();
        }
        if let Some(c) = conn {
            self.detach(c);
        }
        self.asks.retain(|(pid, ..)| *pid != id);
        self.after_end(id);
    }

    fn fail(&mut self, id: PaneId, e: Error) {
        let Some(p) = self.panes.get_mut(&id) else {
            return;
        };
        p.state = State::Failed(clean(&e.to_string()));
        p.outcome = Some(Err(e.into()));
        p.task = None;
        self.asks.retain(|(pid, ..)| *pid != id);
        self.after_end(id);
    }

    fn after_end(&mut self, id: PaneId) {
        if self.exit_with_last
            && self.panes.len() == 1
            && let Some(o) = self.panes.get_mut(&id).and_then(|p| p.outcome.take())
        {
            self.done = Some(o);
        }
    }

    /// Resultado de la aplicación al cerrar el último panel: el de ese panel si solo hubo
    /// uno; con varios, 0.
    fn final_result(&mut self) -> anyhow::Result<i32> {
        if self.opened_total == 1 {
            self.last_outcome.take().unwrap_or(Ok(0))
        } else {
            Ok(0)
        }
    }

    async fn close_pane(&mut self, id: PaneId) {
        let Some(p) = self.panes.get_mut(&id) else {
            return;
        };
        if let Some(o) = p.outcome.take() {
            self.last_outcome = Some(o);
        }
        let (task, conn) = (p.task.take(), p.conn.take());
        if let Some(t) = task {
            t.abort();
        }
        if let Some(c) = conn {
            self.detach(c);
        }
        self.asks.retain(|(pid, ..)| *pid != id);
        if self.panes.len() <= 1 {
            // Último panel: se deja en pantalla para el último dibujo.
            self.done = Some(self.final_result());
            return;
        }
        if let Some(ti) = self.tabs.iter().position(|t| t.root.leaves().contains(&id)) {
            let tab = &mut self.tabs[ti];
            let was = tab.root.leaves().iter().position(|&l| l == id).unwrap_or(0);
            match tab.root.clone().remove(id) {
                Some(mut root) => {
                    root.balance();
                    if tab.focus == id {
                        let rest = root.leaves();
                        let i = was.min(rest.len().saturating_sub(1));
                        tab.focus = rest.get(i).copied().unwrap_or(tab.focus);
                    }
                    tab.root = root;
                }
                None => {
                    self.tabs.remove(ti);
                    if ti < self.cur {
                        self.cur -= 1;
                    }
                    self.cur = self.cur.min(self.tabs.len().saturating_sub(1));
                }
            }
        }
        self.panes.remove(&id);
        self.relayout().await;
    }

    async fn send(&mut self, id: PaneId, bytes: &[u8]) {
        let Some(o) = self.panes.get_mut(&id).and_then(|p| p.out.as_mut()) else {
            return;
        };
        if let Err(e) = o.send(bytes).await {
            self.end_pane(id, Err(e));
        }
    }

    // ---- mensajes de los paneles ----

    async fn on_msg(&mut self, m: Msg) {
        match m {
            Msg::Ask(id, mut d, tx) => {
                if let Some(p) = self.panes.get(&id) {
                    d.title = format!("{} ({}): {}", clean(&p.session.name), p.host, d.title);
                    self.asks.push_back((id, d, tx));
                }
            }
            Msg::Opened(id, o) => self.opened(id, *o).await,
            Msg::Failed(id, e) => self.fail(id, e),
            Msg::Shell(id, ev) => self.shell_event(id, ev),
            Msg::ReaderEnd(id) => self.reader_end(id, None),
        }
    }

    async fn opened(&mut self, id: PaneId, o: Opened) {
        let Some(p) = self
            .panes
            .get_mut(&id)
            .filter(|p| matches!(p.state, State::Connecting))
        else {
            self.detach(o.conn);
            return;
        };
        let (cols, rows) = p.size;
        let mut out = Out {
            writer: o.writer,
            log: Log::open(&p.session, &self.paths.logs_dir, &o.warnings),
            dropped: false,
        };
        // El panel pudo cambiar de tamaño mientras se conectaba.
        let resize_err = if o.size == p.size {
            None
        } else {
            out.resize(cols, rows).await.err()
        };
        p.emu = Some(Emulator::new(rows, cols, SCROLLBACK));
        p.cipher = o
            .negotiated
            .as_ref()
            .map(|n| compact(&n.cipher))
            .unwrap_or_default();
        p.legacy = p.session.legacy_algorithms || o.negotiated.as_ref().is_some_and(is_legacy);
        p.modal = (!o.warnings.is_empty()).then(|| Modal {
            dialog: Dialog::notice("Avisos de la conexión", &o.warnings, true),
            paste: None,
        });
        p.conn = Some(o.conn);
        p.out = Some(out);
        p.state = State::Live;
        if let Some(e) = resize_err {
            self.end_pane(id, Err(e));
        }
    }

    fn shell_event(&mut self, id: PaneId, ev: ShellEvent) {
        let Some(p) = self.panes.get_mut(&id).filter(|p| !p.over()) else {
            return;
        };
        match ev {
            ShellEvent::Data(d) => {
                if let Some(o) = p.out.as_mut() {
                    o.log.output(&d);
                }
                if let Some(e) = p.emu.as_mut() {
                    e.process(&d);
                }
            }
            ev => match exit_code(&ev) {
                Some(c) => p.exit = Some(c), // se sigue drenando hasta Closed/Lost
                None => self.reader_end(id, Some(ev)),
            },
        }
    }

    fn reader_end(&mut self, id: PaneId, last: Option<ShellEvent>) {
        let exit = self.panes.get(&id).and_then(|p| p.exit);
        self.end_pane(id, finish(exit, last));
    }

    // ---- entrada ----

    async fn on_event(&mut self, ev: Event) {
        match ev {
            Event::Key(k) if !pressed(&k) => {}
            Event::Key(k) => self.on_key(k).await,
            Event::Paste(text) => self.on_paste(text).await,
            Event::Resize(w, h) => {
                self.size = (w, h);
                self.relayout().await;
            }
            Event::Mouse(m) => {
                let delta = match m.kind {
                    MouseEventKind::ScrollUp => WHEEL_LINES,
                    MouseEventKind::ScrollDown => -WHEEL_LINES,
                    _ => return,
                };
                self.scroll_focus(delta);
            }
            _ => {}
        }
    }

    fn scroll_focus(&mut self, delta: i32) {
        if let Some(id) = self.focus_id()
            && let Some(e) = self.panes.get_mut(&id).and_then(|p| p.emu.as_mut())
        {
            scroll(e, delta);
        }
    }

    async fn on_key(&mut self, k: KeyEvent) {
        self.notice = None;
        if let Some((_, d, _)) = self.asks.front_mut() {
            if let Some(ans) = d.handle_key(&k)
                && let Some((_, _, reply)) = self.asks.pop_front()
            {
                let _ = reply.send(ans);
            }
            return;
        }
        if let Some(id) = self.focus_id()
            && let Some(p) = self.panes.get_mut(&id)
            && let Some(m) = p.modal.as_mut()
        {
            if let Some(ans) = m.dialog.handle_key(&k) {
                let text = m.paste.take();
                p.modal = None;
                if let (Answer::Confirm(true), Some(t)) = (ans, text) {
                    self.paste_to(id, &t).await;
                }
            }
            return;
        }
        if self.overlay.is_some() {
            self.overlay_key(k).await;
            return;
        }
        match self.keymap.feed(&k) {
            KeyOut::Forward(key, mods) | KeyOut::Literal(key, mods) => {
                self.forward(key, mods).await;
            }
            KeyOut::Do(a) => self.action(a).await,
            KeyOut::Armed | KeyOut::Cancel | KeyOut::Ignore => {}
        }
    }

    async fn overlay_key(&mut self, k: KeyEvent) {
        match self.overlay.take() {
            Some(Overlay::Picker { mut picker, mode }) => match picker.handle_key(&k) {
                PickerOut::Pending => self.overlay = Some(Overlay::Picker { picker, mode }),
                PickerOut::Cancel => {
                    if self.tabs.is_empty() {
                        self.done = Some(Ok(0));
                    }
                }
                PickerOut::Open(s) => self.open(*s, mode).await,
            },
            Some(Overlay::Rename(mut d)) => match d.handle_key(&k) {
                None => self.overlay = Some(Overlay::Rename(d)),
                Some(Answer::Secret(Some(v))) => {
                    let name = clean(v.trim());
                    if let Some(t) = self.tabs.get_mut(self.cur).filter(|_| !name.is_empty()) {
                        t.name = name;
                    }
                }
                Some(_) => {}
            },
            Some(Overlay::Help) | None => {} // cualquier tecla cierra la ayuda
        }
    }

    async fn on_paste(&mut self, text: String) {
        if let Some((_, d, _)) = self.asks.front_mut() {
            d.paste(&text);
            return;
        }
        let Some(id) = self.focus_id() else {
            if let Some(Overlay::Picker { picker, .. }) = self.overlay.as_mut() {
                picker.paste(&text);
            }
            return;
        };
        if let Some(m) = self.panes.get_mut(&id).and_then(|p| p.modal.as_mut()) {
            m.dialog.paste(&text);
            return;
        }
        match self.overlay.as_mut() {
            Some(Overlay::Picker { picker, .. }) => picker.paste(&text),
            Some(Overlay::Rename(d)) => d.paste(&text),
            Some(Overlay::Help) => {}
            None => {
                let Some(p) = self.panes.get_mut(&id) else {
                    return;
                };
                let Some(emu) = p.emu.as_ref().filter(|_| matches!(p.state, State::Live)) else {
                    return;
                };
                if emu.paste_needs_confirmation(&text) {
                    let n = text.trim_end_matches(['\n', '\r']).lines().count();
                    p.modal = Some(Modal {
                        dialog: Dialog::paste_confirm(n),
                        paste: Some(text),
                    });
                } else {
                    self.paste_to(id, &text).await;
                }
            }
        }
    }

    async fn paste_to(&mut self, id: PaneId, text: &str) {
        let Some(emu) = self.panes.get(&id).and_then(|p| p.emu.as_ref()) else {
            return;
        };
        let bytes = emu.encode_paste(text);
        self.send(id, &bytes).await;
    }

    /// Tecla normal (o prefijo literal): al remoto si el panel está vivo.
    async fn forward(&mut self, key: Key, mods: Modifiers) {
        let Some(id) = self.focus_id() else { return };
        let Some(p) = self.panes.get_mut(&id) else {
            return;
        };
        match p.state {
            State::Live => {
                let bytes = p
                    .emu
                    .as_mut()
                    .map(|e| {
                        let b = e.encode_key(key, mods);
                        if e.scroll_offset() != 0 {
                            e.scroll_view(0);
                        }
                        b
                    })
                    .unwrap_or_default();
                self.send(id, &bytes).await;
            }
            State::Connecting => {
                if mods.ctrl && key == Key::Char('c') {
                    p.outcome = Some(Ok(130)); // cancelado por el usuario
                    self.close_pane(id).await;
                }
            }
            State::Ended(_) | State::Failed(_) => self.close_pane(id).await,
        }
    }

    async fn action(&mut self, a: Action) {
        let has_tabs = !self.tabs.is_empty();
        match a {
            Action::Quit => self.done = Some(Ok(0)),
            Action::Help => self.overlay = Some(Overlay::Help),
            Action::NewTab | Action::Sessions => self.open_picker(Mode::Tab),
            Action::SplitHorizontal if has_tabs => self.open_picker(Mode::Split(Dir::Horizontal)),
            Action::SplitVertical if has_tabs => self.open_picker(Mode::Split(Dir::Vertical)),
            Action::ClosePane => {
                if let Some(id) = self.focus_id() {
                    self.close_pane(id).await;
                }
            }
            Action::NextTab | Action::PrevTab if has_tabs => {
                let n = self.tabs.len();
                self.cur = if a == Action::NextTab {
                    (self.cur + 1) % n
                } else {
                    (self.cur + n - 1) % n
                };
            }
            Action::FocusNext | Action::FocusPrev => {
                if let Some(tab) = self.tabs.get_mut(self.cur) {
                    let d = if a == Action::FocusNext { 1 } else { -1 };
                    tab.focus = cycle(&tab.root.leaves(), tab.focus, d);
                }
            }
            Action::ScrollUp | Action::ScrollDown => {
                let page = self
                    .focus_id()
                    .and_then(|id| self.panes.get(&id))
                    .map_or(1, |p| i32::from(p.size.1.saturating_sub(1).max(1)));
                self.scroll_focus(if a == Action::ScrollUp { page } else { -page });
            }
            Action::RenameTab => {
                if let Some(t) = self.tabs.get(self.cur) {
                    self.overlay = Some(Overlay::Rename(Dialog::text(
                        "Renombrar pestaña",
                        "Nombre: ",
                        &t.name,
                    )));
                }
            }
            Action::Search => self.notice = Some("búsqueda: pendiente".into()),
            Action::CopyMode => self.notice = Some("modo copia: pendiente".into()),
            _ => {}
        }
    }

    // ---- dibujo ----

    fn draw<B>(&self, tui: &mut Terminal<B>) -> anyhow::Result<()>
    where
        B: Backend,
        B::Error: std::error::Error + Send + Sync + 'static,
    {
        tui.draw(|f| self.render(f))?;
        Ok(())
    }

    fn state_text(&self, p: &Pane) -> String {
        if let Some(n) = &self.notice {
            return n.clone();
        }
        match (&p.state, &p.emu) {
            (State::Connecting, _) => "conectando...".into(),
            (State::Failed(_), _) => "error de conexión".into(),
            (State::Ended(_), _) => "terminada".into(),
            (State::Live, emu) => {
                if p.out.as_ref().is_some_and(|o| o.dropped) {
                    "ENTRADA DESCARTADA (cola llena)".into()
                } else if self.keymap.is_armed() {
                    "prefijo ...".into()
                } else if emu.as_ref().is_some_and(|e| e.alternate_screen()) {
                    "pantalla alternativa".into()
                } else if let Some(e) = emu.as_ref().filter(|e| e.scroll_offset() > 0) {
                    format!("scrollback {}/{}", e.scroll_offset(), e.scrollback_len())
                } else {
                    "conectado".into()
                }
            }
        }
    }

    fn render(&self, f: &mut Frame) {
        let area = f.area();
        if area.height == 0 {
            return;
        }
        let tabbar = self.tabs.len() > 1;
        if tabbar {
            let mut spans = vec![];
            for (i, t) in self.tabs.iter().enumerate() {
                let style = if i == self.cur {
                    Style::default()
                        .fg(Color::Black)
                        .bg(Color::Cyan)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::Black).bg(Color::Gray)
                };
                spans.push(Span::styled(format!(" {}:{} ", i + 1, t.name), style));
                spans.push(Span::raw(" "));
            }
            f.render_widget(
                Paragraph::new(Line::from(spans)).style(Style::default().bg(Color::DarkGray)),
                Rect { height: 1, ..area },
            );
        }
        let body = body_of(area.width, area.height, self.tabs.len());
        let focus = self.focus_id();
        let modal_open = !self.asks.is_empty()
            || self.overlay.is_some()
            || focus
                .and_then(|id| self.panes.get(&id))
                .is_some_and(|p| p.modal.is_some());
        let mut cursor = None;
        if let Some(tab) = self.tabs.get(self.cur) {
            let (panes, seps) = compute(tab, body);
            let multi = panes.len() > 1;
            for (id, r) in panes {
                let Some(p) = self.panes.get(&id) else {
                    continue;
                };
                let focused = Some(id) == focus;
                let c = self.render_pane(f, p, r, multi, focused);
                if focused {
                    cursor = c;
                }
            }
            for s in seps {
                for y in s.y..s.bottom() {
                    if let Some(c) = f.buffer_mut().cell_mut((s.x, y)) {
                        c.set_symbol("│")
                            .set_style(Style::default().fg(Color::DarkGray));
                    }
                }
            }
        }
        // Barra de estado del panel con foco.
        let bar = Rect {
            y: area.bottom() - 1,
            height: 1,
            ..area
        };
        let hint = self.keymap.hint();
        let line = match focus.and_then(|id| self.panes.get(&id)) {
            Some(p) => {
                let state = self.state_text(p);
                let info = StatusInfo {
                    session: &p.session.name,
                    host: &p.host,
                    state: &state,
                    cipher: &p.cipher,
                    legacy: p.legacy,
                    log_warn: p.out.as_ref().and_then(|o| o.log.warn.as_deref()),
                };
                status_line(&info, &hint, bar.width)
            }
            None => {
                let info = StatusInfo {
                    state: "sin sesiones",
                    ..Default::default()
                };
                status_line(&info, &hint, bar.width)
            }
        };
        f.render_widget(Paragraph::new(line), bar);
        // Diálogos: los de conexión primero (de cualquier panel), luego los del panel.
        if let Some((_, d, _)) = self.asks.front() {
            draw_dialog(f, d);
        } else if let Some(m) = focus
            .and_then(|id| self.panes.get(&id))
            .and_then(|p| p.modal.as_ref())
        {
            draw_dialog(f, &m.dialog);
        } else {
            match &self.overlay {
                Some(Overlay::Help) => popup(f, "Ayuda", lines(self.keymap.help_lines()), false),
                Some(Overlay::Picker { picker, .. }) => {
                    popup(f, "Abrir sesión", lines(picker.lines()), false);
                }
                Some(Overlay::Rename(d)) => draw_dialog(f, d),
                None => {}
            }
        }
        if !modal_open && let Some(pos) = cursor {
            f.set_cursor_position(pos);
        }
    }

    /// Dibuja un panel en `r`; devuelve la posición del cursor si es visible.
    fn render_pane(
        &self,
        f: &mut Frame,
        p: &Pane,
        r: Rect,
        multi: bool,
        focused: bool,
    ) -> Option<(u16, u16)> {
        let area = inner(r, multi);
        let screen = p.emu.as_ref().map(Emulator::screen);
        if multi {
            let remote = screen.as_ref().map(|s| clean(&s.title)).unwrap_or_default();
            let title = if remote.is_empty() {
                clean(&p.session.name)
            } else {
                format!("{} - {remote}", clean(&p.session.name))
            };
            let style = if focused {
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::Black).bg(Color::Gray)
            };
            f.render_widget(
                Paragraph::new(format!(" {title} ({}) ", p.host)).style(style),
                Rect { height: 1, ..r },
            );
        }
        let note = |f: &mut Frame, text: String, style: Style, row: u16| {
            if area.height > row {
                f.render_widget(
                    Paragraph::new(text).style(style),
                    Rect {
                        y: area.y + row,
                        height: 1,
                        ..area
                    },
                );
            }
        };
        let plain = Style::default();
        let rev = Style::default().add_modifier(Modifier::REVERSED);
        match (&p.state, &screen) {
            (State::Connecting, _) => {
                note(f, format!("conectando a {}...", p.host), plain, 0);
                note(f, "(Ctrl+C cancela)".into(), plain, 1);
                None
            }
            (State::Failed(msg), _) => {
                note(f, format!("error: {msg}"), plain, 0);
                note(f, "Pulse una tecla para cerrar el panel.".into(), rev, 2);
                None
            }
            (state, Some(s)) => {
                screen_to_buffer(s, f.buffer_mut(), area);
                if let State::Ended(label) = state {
                    let row = area.height.saturating_sub(1);
                    note(
                        f,
                        format!("{label} - pulse una tecla para cerrar"),
                        rev,
                        row,
                    );
                    return None;
                }
                (s.cursor_visible && s.cursor.0 < area.height && s.cursor.1 < area.width)
                    .then_some((area.x + s.cursor.1, area.y + s.cursor.0))
            }
            _ => None,
        }
    }

    /// Cierra todas las conexiones (con un plazo) al salir.
    async fn shutdown(&mut self) {
        let mut handles = std::mem::take(&mut self.disconnects);
        for p in self.panes.values_mut() {
            if let Some(t) = p.task.take() {
                t.abort();
            }
            if let Some(c) = p.conn.take() {
                handles.push(tokio::spawn(async move {
                    let _ = c.disconnect().await;
                }));
            }
        }
        let _ = tokio::time::timeout(
            Duration::from_secs(2),
            futures_util::future::join_all(handles),
        )
        .await;
    }
}

fn lines(v: Vec<String>) -> Vec<Line<'static>> {
    v.into_iter().map(Line::from).collect()
}

pub(crate) async fn run_loop<B, S>(
    tui: &mut Terminal<B>,
    mut events: S,
    initial: Option<SessionConfig>,
    paths: &Paths,
    exit_with_last: bool,
) -> anyhow::Result<i32>
where
    B: Backend,
    B::Error: std::error::Error + Send + Sync + 'static,
    S: Stream<Item = std::io::Result<Event>> + Unpin,
{
    let (tx, mut rx) = mpsc::unbounded_channel::<Msg>();
    let mut app = App::new(tx, paths, exit_with_last);
    let size = tui.size()?;
    app.size = (size.width, size.height);
    match initial {
        Some(s) => app.open(s, Mode::Tab).await,
        None => app.open_picker(Mode::Tab),
    }
    let mut tick = tokio::time::interval(Duration::from_millis(16));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut dirty = true;
    let res = loop {
        tokio::select! {
            ev = events.next() => match ev {
                None => break Ok(1), // se cerró la fuente de eventos (terminal desaparecida)
                Some(Err(e)) => break Err(e.into()),
                Some(Ok(ev)) => {
                    app.on_event(ev).await;
                    dirty = true;
                }
            },
            Some(m) = rx.recv() => {
                app.on_msg(m).await;
                dirty = true;
            }
            _ = tick.tick() => {
                if dirty {
                    dirty = false;
                    app.draw(tui)?;
                }
            }
        }
        if let Some(r) = app.done.take() {
            break r;
        }
    };
    // Último dibujo con lo que llegó justo antes de cerrar.
    let _ = app.draw(tui);
    app.shutdown().await;
    res
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEventKind, KeyModifiers};
    use futures_util::stream;
    use ratatui::backend::TestBackend;

    fn key(c: KeyCode) -> Event {
        Event::Key(KeyEvent::new(c, KeyModifiers::NONE))
    }

    fn typed(s: &str) -> Vec<(u64, Event)> {
        s.chars()
            .map(|c| {
                (
                    0,
                    key(if c == '\n' {
                        KeyCode::Enter
                    } else {
                        KeyCode::Char(c)
                    }),
                )
            })
            .collect()
    }

    fn script(items: Vec<(u64, Event)>) -> impl Stream<Item = std::io::Result<Event>> + Unpin {
        Box::pin(stream::unfold(items.into_iter(), |mut it| async move {
            let (ms, ev) = it.next()?;
            tokio::time::sleep(Duration::from_millis(ms)).await;
            Some((Ok(ev), it))
        }))
    }

    fn paths() -> Paths {
        let d = std::env::temp_dir().join(format!("friki-tui-app-{}", std::process::id()));
        Paths {
            known_hosts: d.join("known_hosts"),
            logs_dir: d.join("logs"),
            sessions_dir: d.join("sessions"),
        }
    }

    fn screen_text(t: &Terminal<TestBackend>) -> String {
        t.backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect()
    }

    #[tokio::test]
    async fn picker_esc_without_sessions_exits_zero() {
        let mut tui = Terminal::new(TestBackend::new(80, 20)).expect("terminal");
        let ev = script(vec![(0, key(KeyCode::Esc))]);
        let r = run_with_test(&mut tui, ev, None).await;
        assert_eq!(r.expect("salida"), 0);
        assert!(screen_text(&tui).contains("sin sesiones"));
    }

    async fn run_with_test(
        tui: &mut Terminal<TestBackend>,
        ev: impl Stream<Item = std::io::Result<Event>> + Unpin,
        initial: Option<SessionConfig>,
    ) -> anyhow::Result<i32> {
        crate::run_with(tui, ev, initial, &paths(), false).await
    }

    #[tokio::test]
    async fn invalid_target_keeps_picker_open_and_closed_input_exits_one() {
        let mut tui = Terminal::new(TestBackend::new(80, 20)).expect("terminal");
        let ev = script(typed("no valido\n"));
        let r = run_with_test(&mut tui, ev, None).await;
        assert_eq!(r.expect("salida"), 1); // fuente de eventos agotada
        let text = screen_text(&tui);
        assert!(text.contains("Abrir sesión") && text.contains("destino no válido"));
    }

    #[tokio::test]
    async fn connection_failure_shows_error_and_closes_with_a_key() {
        let mut tui = Terminal::new(TestBackend::new(90, 20)).expect("terminal");
        let mut evs = typed("127.0.0.1:1\n");
        evs.push((800, key(KeyCode::Char('x'))));
        let r = run_with_test(&mut tui, script(evs), None).await;
        // Un solo panel: se propaga el error de friki_ssh sin envolver.
        let e = r.expect_err("fallo de conexión");
        assert!(e.downcast_ref::<Error>().is_some(), "{e:#}");
        let text = screen_text(&tui);
        assert!(
            text.contains("error:") && text.contains("cerrar el panel"),
            "{text}"
        );
    }

    #[tokio::test]
    async fn failed_pane_does_not_close_the_app_and_help_opens() {
        let mut tui = Terminal::new(TestBackend::new(100, 30)).expect("terminal");
        let pre = Event::Key(KeyEvent::new(KeyCode::Char(']'), KeyModifiers::CONTROL));
        let mut evs = typed("127.0.0.1:1\n");
        // Abre otra pestaña (picker) y la cancela: el panel fallido sigue ahí.
        evs.push((600, pre.clone()));
        evs.push((0, key(KeyCode::Char('c'))));
        evs.push((0, key(KeyCode::Esc)));
        evs.push((0, pre.clone()));
        evs.push((0, key(KeyCode::Char('?'))));
        let r = run_with_test(&mut tui, script(evs), None).await;
        assert_eq!(r.expect("salida"), 1); // entrada agotada con la ayuda abierta
        let text = screen_text(&tui);
        assert!(
            text.contains("Ayuda") && text.contains("ctrl+] x"),
            "{text}"
        );
    }

    #[test]
    fn release_events_are_not_pressed() {
        let mut k = KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE);
        k.kind = KeyEventKind::Release;
        assert!(!pressed(&k));
    }
}
