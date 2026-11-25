//! Aplicación: pestañas, paneles (una sesión cada uno) y el bucle único.
//!
//! Cada panel lanza una tarea que conecta, abre el shell y reenvía todo (diálogos de
//! conexión, eventos del shell) por un único canal a este bucle. Los diálogos de clave
//! de host y autenticación se muestran siempre de forma modal, con el nombre de la
//! sesión a la que pertenecen, aunque otro panel tenga el foco.

use std::collections::{BTreeMap, VecDeque};
use std::future::Future;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crossterm::event::{Event, KeyEvent, MouseEventKind};
use friki_core::config::{
    AppConfig, Protocol, SessionConfig, load_app_config, load_sessions, resolve_target,
};
use friki_core::known_hosts::{HostKey, HostKeyStatus};
use friki_core::transport::{StreamEvent, StreamReader, StreamWriter};
use friki_script::{CancelToken, Limits, Permissions, ScriptError};
use friki_ssh::{
    AuthPrompter, Connection, Error, HostKeyDecision, KnownHostsVerifier, Negotiated, Zeroizing,
};
use friki_term::{Key, Modifiers, Terminal as Emulator};
use friki_zmodem::{FileProtocol, Progress, ZmodemStart};
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

use crate::copy_mode::{Clipboard, CopyMode, CopyOutcome, copy_to_clipboard};
use crate::dialog::{Answer, Dialog, clean};
use crate::input::pressed;
use crate::keys::{Action, KeyOut, Keymap};
use crate::layout::{Dir, Node, PaneId, cycle};
use crate::picker::{Picker, PickerOut};
use crate::render::{StatusInfo, draw_dialog, popup, screen_to_buffer, status_line};
use crate::script_host::{HostReq, PaneHost, Shared};
use crate::search::{Search, SearchOutcome};
use crate::target::{forward_label, label, telnet_options, term_for};
use crate::theme::Theme;
use crate::xfer::{CANCEL, PaneIo, bar};
use crate::{Log, Out, Paths, WHEEL_LINES, compact, exit_code, finish, is_legacy};

type Ask = Pin<Box<dyn Future<Output = HostKeyDecision> + Send>>;

/// Lo que una tarea de panel envía al bucle central.
enum Msg {
    Ask(PaneId, Dialog, oneshot::Sender<Answer>),
    Opened(PaneId, Box<Opened>),
    /// Fallo al conectar: un `friki_ssh::Error` en SSH, un error de red genérico en el resto.
    Failed(PaneId, anyhow::Error),
    Stream(PaneId, StreamEvent),
    ReaderEnd(PaneId),
    Xfer(PaneId, XferMsg),
    Script(PaneId, HostReq),
    ScriptDone(PaneId, Result<i64, ScriptError>),
}

/// Lo que la tarea de una transferencia Zmodem cuenta al bucle.
enum XferMsg {
    Progress(Progress),
    /// Ficheros y bytes sobrantes tras el protocolo; el adaptador vuelve para devolver su cola.
    Done(std::io::Result<(Vec<PathBuf>, Vec<u8>)>, Box<PaneIo>),
}

struct Opened {
    /// Solo SSH: la conexión que mantiene vivas las redirecciones y el canal.
    conn: Option<Connection>,
    writer: Arc<dyn StreamWriter>,
    /// (columnas, filas) con las que se abrió el PTY.
    size: (u16, u16),
    warnings: Vec<String>,
    negotiated: Option<Negotiated>,
    /// Redirecciones SSH activas (ya formateadas) y las que fallaron (con su motivo).
    forwards: Vec<String>,
    fwd_errors: Vec<String>,
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
        self.keyboard_interactive_for(&self.host, name, instructions, prompts)
            .await
    }

    /// Muestra el servidor que de verdad pregunta (un salto de ProxyJump o el destino).
    async fn keyboard_interactive_for(
        &self,
        host: &str,
        name: &str,
        instructions: &str,
        prompts: &[(String, bool)],
    ) -> Option<Vec<Zeroizing<String>>> {
        match self
            .ui
            .ask(Dialog::keyboard_interactive(
                host,
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

/// Error de red genérico para los transportes que no son SSH.
fn net(e: &std::io::Error) -> anyhow::Error {
    Error::Network(e.to_string()).into()
}

/// SSH: verificación de clave de host y autenticación por diálogos, shell con PTY y las
/// redirecciones de la sesión (cada resultado queda para el panel; un fallo no tumba el shell).
async fn open_ssh(
    ui: UiChannel,
    session: &SessionConfig,
    known_hosts: std::path::PathBuf,
    (cols, rows): (u16, u16),
    term: &str,
) -> anyhow::Result<(Opened, Box<dyn StreamReader>)> {
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
    let conn = Connection::connect(session, verifier, prompter).await?;
    let (writer, reader) = conn.open_shell_stream(term, cols, rows).await?;
    let (mut forwards, mut fwd_errors) = (vec![], vec![]);
    for f in &session.forwards {
        match conn.start_forward(f).await {
            Ok(h) => forwards.push(forward_label(f, Some(h.listen))),
            Err(e) => fwd_errors.push(format!(
                "redirección {}: {}",
                forward_label(f, None),
                clean(&e.to_string())
            )),
        }
    }
    let opened = Opened {
        warnings: conn.warnings(),
        negotiated: conn.negotiated(),
        conn: Some(conn),
        writer,
        size: (cols, rows),
        forwards,
        fwd_errors,
    };
    Ok((opened, reader))
}

/// Abre el flujo según el protocolo de la sesión.
async fn open_stream(
    ui: UiChannel,
    session: &SessionConfig,
    known_hosts: std::path::PathBuf,
    size: (u16, u16),
) -> anyhow::Result<(Opened, Box<dyn StreamReader>)> {
    let term = term_for(session);
    let (cols, rows) = size;
    let stream = match session.protocol {
        Protocol::Ssh => return open_ssh(ui, session, known_hosts, size, &term).await,
        Protocol::Telnet | Protocol::TelnetTls | Protocol::Raw => {
            let opts = telnet_options(session, &term);
            friki_telnet::connect(&session.host, session.port, &opts, cols, rows).await
        }
        Protocol::Serial => friki_serial::open(&session.serial).await,
        Protocol::LocalShell => {
            friki_pty::spawn(session.local_command.as_deref(), &term, cols, rows).await
        }
    }
    .map_err(|e| net(&e))?;
    let (writer, reader) = stream;
    let opened = Opened {
        conn: None,
        writer,
        size,
        warnings: vec![],
        negotiated: None,
        forwards: vec![],
        fwd_errors: vec![],
    };
    Ok((opened, reader))
}

/// Conecta, abre el flujo y reenvía sus eventos hasta que se agota.
fn spawn_connect(
    tx: mpsc::UnboundedSender<Msg>,
    id: PaneId,
    session: SessionConfig,
    known_hosts: std::path::PathBuf,
    size: (u16, u16),
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let ui = UiChannel {
            tx: tx.clone(),
            pane: id,
        };
        let (opened, mut reader) = match open_stream(ui, &session, known_hosts, size).await {
            Ok(x) => x,
            Err(e) => {
                let _ = tx.send(Msg::Failed(id, e));
                return;
            }
        };
        if tx.send(Msg::Opened(id, Box::new(opened))).is_err() {
            return;
        }
        while let Some(ev) = reader.next_event().await {
            if tx.send(Msg::Stream(id, ev)).is_err() {
                return;
            }
        }
        let _ = tx.send(Msg::ReaderEnd(id));
    })
}

/// Qué hacer con la respuesta de un diálogo modal del panel.
enum Then {
    Nothing,
    /// Texto a pegar si se confirma.
    Paste(String),
    ZmodemRecv,
    /// Rutas locales (separadas por `;`) a enviar por Zmodem.
    ZmodemSend,
    /// Recepción Kermit iniciada por el usuario (Kermit no tiene señal de arranque).
    KermitRecv,
    /// Rutas locales (separadas por `;`) a enviar por Kermit.
    KermitSend,
    /// Script a ejecutar (nombre dentro del directorio de scripts o ruta).
    Script,
}

/// Diálogo modal de un panel.
struct Modal {
    dialog: Dialog,
    then: Then,
}

impl Modal {
    fn notice(title: &str, lines: &[String], alert: bool) -> Self {
        Self {
            dialog: Dialog::notice(title, lines, alert),
            then: Then::Nothing,
        }
    }
}

/// Transferencia en curso (Zmodem o Kermit): el flujo del panel se desvía a `tx`.
struct Xfer {
    tx: mpsc::UnboundedSender<Vec<u8>>,
    task: JoinHandle<()>,
    receiving: bool,
    /// Nombre del protocolo para los avisos.
    name: &'static str,
    progress: Option<Progress>,
    /// Directorio de descargas (para avisar del fichero parcial al cancelar).
    dir: PathBuf,
}

impl Xfer {
    fn line(&self, width: u16) -> String {
        let what = if self.receiving {
            "recibiendo"
        } else {
            "enviando"
        };
        let tail = " (Esc cancela)";
        let text = match &self.progress {
            Some(p) => format!(
                "{} {what} {} {}{tail}",
                self.name,
                clean(&p.file),
                bar(p.done, p.total, 20)
            ),
            None => format!("{} {what}...{tail}", self.name),
        };
        text.chars().take(usize::from(width)).collect()
    }
}

/// Script en ejecución sobre un panel.
struct ScriptRun {
    shared: Arc<Shared>,
    task: JoinHandle<()>,
    cancel: CancelToken,
    cancelled: bool,
}

/// Tras un rechazo del diálogo Zmodem, durante este tiempo no se vuelve a preguntar
/// (el remoto repite su petición).
const ZMODEM_QUIET: Duration = Duration::from_secs(10);
/// Tope de lo retenido mientras el diálogo Zmodem espera respuesta.
const ZMODEM_HOLD: usize = 64 * 1024;
/// Tope de bytes que una recepción Zmodem puede escribir en disco.
const ZMODEM_MAX_BYTES: u64 = 4 * 1024 * 1024 * 1024;
/// Un diálogo que abre el remoto descarta el teclado este tiempo.
const DIALOG_LOCK: Duration = Duration::from_secs(1);

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
    /// Redirecciones SSH activas, para la barra de estado.
    forwards: Vec<String>,
    theme: Theme,
    modal: Option<Modal>,
    conn: Option<Connection>,
    task: Option<JoinHandle<()>>,
    exit: Option<i32>,
    outcome: Option<anyhow::Result<i32>>,
    search: Option<Search>,
    copy: Option<CopyMode>,
    xfer: Option<Xfer>,
    script: Option<ScriptRun>,
    /// Bytes de la petición Zmodem retenidos mientras se espera la respuesta del usuario
    /// (no llegan al emulador; se reenvían a la transferencia o, si se rechaza, al emulador).
    zm_hold: Option<Vec<u8>>,
    zm_quiet: Option<Instant>,
    /// Últimos bytes del flujo, por si la señal Zmodem llega partida entre dos lecturas.
    zm_tail: Vec<u8>,
    /// Un `connect` de script a la espera de que el panel abra.
    pending_connect: Option<oneshot::Sender<Result<(), String>>>,
}

impl Pane {
    fn new(session: SessionConfig) -> Self {
        Self {
            host: label(&session),
            session,
            state: State::Connecting,
            emu: None,
            out: None,
            size: (80, 24),
            cipher: String::new(),
            legacy: false,
            forwards: vec![],
            theme: Theme::default(),
            modal: None,
            conn: None,
            task: None,
            exit: None,
            outcome: None,
            search: None,
            copy: None,
            xfer: None,
            script: None,
            zm_hold: None,
            zm_quiet: None,
            zm_tail: vec![],
            pending_connect: None,
        }
    }

    /// Cancela lo que dependa del flujo (transferencia y script) al cerrarse el panel.
    fn stop_jobs(&mut self) {
        if let Some(x) = self.xfer.take() {
            x.task.abort();
        }
        if let Some(s) = self.script.take() {
            s.shared.cancel();
            s.cancel.cancel();
            s.task.abort();
        }
    }

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
    /// Avisos de la configuración global; se muestran una vez (selector o primer panel).
    config_warnings: Vec<String>,
    /// Salida de la terminal anfitriona (OSC 52).
    host_out: Box<dyn Write + Send>,
    /// Portapapeles del sistema (inyectable en las pruebas).
    clip: fn(&str) -> Result<Clipboard, String>,
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
    /// Carga `config.toml` junto a las sesiones (`app_config_file()` con las rutas por
    /// defecto). Un error de lectura o de contenido es un aviso y se usan los valores por
    /// defecto; atajos inválidos o repetidos, otro aviso y se ignoran.
    fn new(
        tx: mpsc::UnboundedSender<Msg>,
        paths: &'a Paths,
        exit_with_last: bool,
        host_out: Box<dyn Write + Send>,
    ) -> Self {
        let cfg_path = paths
            .sessions_dir
            .parent()
            .map_or_else(friki_core::paths::app_config_file, |d| {
                d.join("config.toml")
            });
        let mut config_warnings = vec![];
        let cfg = load_app_config(&cfg_path).unwrap_or_else(|e| {
            config_warnings.push(format!("configuración: {}", clean(&e.to_string())));
            AppConfig::default()
        });
        let keymap = Keymap::new(&cfg);
        config_warnings.extend(keymap.warnings().iter().map(|w| clean(w)));
        Self {
            tabs: vec![],
            cur: 0,
            panes: BTreeMap::new(),
            next_id: 1,
            keymap,
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
            config_warnings,
            host_out,
            clip: copy_to_clipboard,
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
        // Avisos de configuración y error de sesiones se muestran juntos.
        let mut msgs: Vec<String> = err.into_iter().collect();
        msgs.append(&mut self.config_warnings);
        let err = (!msgs.is_empty()).then(|| msgs.join("; "));
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
        let pane = Pane::new(session.clone());
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
                if let Some(o) = p.out.as_mut() {
                    o.resize(size.0, size.1).await;
                }
            }
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
        if self.end_session(id, outcome) {
            self.after_end(id);
        }
    }

    /// Termina solo la sesión del panel (que sigue abierto); `false` si ya había terminado.
    fn end_session(&mut self, id: PaneId, outcome: anyhow::Result<i32>) -> bool {
        let Some(p) = self.panes.get_mut(&id) else {
            return false;
        };
        if p.over() {
            return false;
        }
        p.state = State::Ended(ended_label(&outcome));
        p.outcome = Some(outcome);
        p.out = None;
        if let Some(x) = p.xfer.take() {
            x.task.abort();
        }
        p.zm_hold = None;
        if let Some(s) = &p.script {
            s.shared.set_writer(None);
        }
        let (task, conn) = (p.task.take(), p.conn.take());
        if let Some(t) = task {
            t.abort();
        }
        if let Some(c) = conn {
            self.detach(c);
        }
        self.asks.retain(|(pid, ..)| *pid != id);
        true
    }

    fn fail(&mut self, id: PaneId, e: anyhow::Error) {
        let Some(p) = self.panes.get_mut(&id) else {
            return;
        };
        let msg = clean(&e.to_string());
        if let Some(r) = p.pending_connect.take() {
            let _ = r.send(Err(msg.clone()));
        }
        p.state = State::Failed(msg);
        p.outcome = Some(Err(e));
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
        p.stop_jobs();
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
        o.send(bytes).await;
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
            Msg::Stream(id, ev) => self.stream_event(id, ev),
            Msg::ReaderEnd(id) => self.reader_end(id, None),
            Msg::Xfer(id, m) => self.xfer_msg(id, m),
            Msg::Script(id, r) => self.script_req(id, r).await,
            Msg::ScriptDone(id, r) => self.script_done(id, r),
        }
    }

    async fn opened(&mut self, id: PaneId, o: Opened) {
        let Some(p) = self
            .panes
            .get_mut(&id)
            .filter(|p| matches!(p.state, State::Connecting))
        else {
            if let Some(c) = o.conn {
                self.detach(c);
            }
            return;
        };
        let (cols, rows) = p.size;
        let emu = Emulator::with_profile(rows, cols, &p.session.terminal);
        let (theme, theme_warn) = Theme::named(&p.session.terminal.theme);
        let mut warnings = o.warnings;
        warnings.extend(o.fwd_errors);
        warnings.extend(emu.profile_warnings());
        warnings.extend(theme_warn);
        warnings.append(&mut self.config_warnings);
        let mut out = Out {
            writer: o.writer,
            log: Log::open(&p.session, &self.paths.logs_dir, &warnings),
            dropped: false,
        };
        // El panel pudo cambiar de tamaño mientras se conectaba.
        if o.size != p.size {
            out.resize(cols, rows).await;
        }
        p.emu = Some(emu);
        p.theme = theme;
        p.forwards = o.forwards;
        p.cipher = o
            .negotiated
            .as_ref()
            .map(|n| compact(&n.cipher))
            .unwrap_or_default();
        p.legacy = p.session.legacy_algorithms || o.negotiated.as_ref().is_some_and(is_legacy);
        p.modal = (!warnings.is_empty()).then(|| Modal {
            dialog: Dialog::notice("Avisos de la conexión", &warnings, true),
            then: Then::Nothing,
        });
        p.conn = o.conn;
        p.out = Some(out);
        p.state = State::Live;
        if let Some(r) = p.pending_connect.take() {
            let _ = r.send(Ok(()));
        }
        if let Some(sc) = &p.script {
            sc.shared
                .set_writer(p.out.as_ref().map(|o| o.writer.clone()));
            if let Some(e) = &p.emu {
                sc.shared.set_screen(&e.screen());
            }
        }
    }

    fn stream_event(&mut self, id: PaneId, ev: StreamEvent) {
        let Some(p) = self.panes.get_mut(&id).filter(|p| !p.over()) else {
            return;
        };
        match ev {
            StreamEvent::Data(d) => {
                if let Some(x) = &p.xfer {
                    let _ = x.tx.send(d); // transferencia: el flujo va a Zmodem, no al emulador
                    return;
                }
                if let Some(h) = p.zm_hold.as_mut() {
                    h.extend_from_slice(&d);
                    if h.len() > ZMODEM_HOLD {
                        abandon_hold(p); // el remoto sigue enviando sin esperar respuesta
                    }
                    return;
                }
                let quiet = p.zm_quiet.is_some_and(|t| t > Instant::now());
                if matches!(p.state, State::Live)
                    && p.modal.is_none()
                    && !quiet
                    && let Some(start) = zmodem_start(&p.zm_tail, &d)
                {
                    let mut hold = std::mem::take(&mut p.zm_tail);
                    hold.extend_from_slice(&d);
                    p.zm_hold = Some(hold);
                    p.modal = Some(match start {
                        ZmodemStart::RemoteSends => Modal {
                            dialog: Dialog::confirm_yes(
                                "Zmodem",
                                &format!(
                                    "Recibir ficheros por Zmodem en {}?",
                                    clean(&self.paths.downloads_dir.display().to_string())
                                ),
                            )
                            .locked_for(DIALOG_LOCK),
                            then: Then::ZmodemRecv,
                        },
                        ZmodemStart::RemoteReceives => Modal {
                            dialog: send_dialog("Zmodem", "El remoto espera ficheros (rz).")
                                .locked_for(DIALOG_LOCK),
                            then: Then::ZmodemSend,
                        },
                    });
                    return;
                }
                deliver(p, &d);
            }
            ev => match exit_code(&ev) {
                Some(c) => p.exit = Some(c), // se sigue drenando hasta Closed/Lost
                None => self.reader_end(id, Some(ev)),
            },
        }
    }

    fn reader_end(&mut self, id: PaneId, last: Option<StreamEvent>) {
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
                let then = std::mem::replace(&mut m.then, Then::Nothing);
                p.modal = None;
                self.modal_answer(id, then, ans).await;
            }
            return;
        }
        if self.overlay.is_some() {
            self.overlay_key(k).await;
            return;
        }
        if self.mode_key(&k) {
            return;
        }
        let armed = self.keymap.is_armed();
        match self.keymap.feed(&k) {
            KeyOut::Forward(key, mods) | KeyOut::Literal(key, mods) => {
                self.forward(key, mods).await;
            }
            KeyOut::Do(a) => self.action(a).await,
            // Prefijo + Ctrl+C: cancela el script del panel con foco.
            KeyOut::Cancel if armed && is_ctrl_c(&k) => self.cancel_script(),
            KeyOut::Armed | KeyOut::Cancel | KeyOut::Ignore => {}
        }
    }

    /// Búsqueda y modo copia: mientras están activos, el teclado es suyo (no va al remoto).
    fn mode_key(&mut self, k: &KeyEvent) -> bool {
        let Some(p) = self.focus_id().and_then(|id| self.panes.get_mut(&id)) else {
            return false;
        };
        let Some(emu) = p.emu.as_mut() else {
            return false;
        };
        if let Some(sr) = p.search.as_mut() {
            if sr.handle_key(k, emu) == SearchOutcome::Exit {
                p.search = None;
            }
            return true;
        }
        if let Some(cm) = p.copy.as_mut() {
            match cm.handle_key(k, emu) {
                CopyOutcome::Continue => {}
                CopyOutcome::Cancel => {
                    p.copy = None;
                    emu.scroll_view(0);
                }
                CopyOutcome::Copy(text) => {
                    p.copy = None;
                    emu.scroll_view(0);
                    self.copy_text(&text);
                }
            }
            return true;
        }
        false
    }

    /// Copia: portapapeles del sistema o, si falla, OSC 52 a la terminal anfitriona.
    fn copy_text(&mut self, text: &str) {
        self.notice = Some(match (self.clip)(text) {
            Ok(Clipboard::System) => "copiado al portapapeles".into(),
            Ok(Clipboard::Osc52(seq)) => {
                let w = &mut self.host_out;
                match w.write_all(seq.as_bytes()).and_then(|()| w.flush()) {
                    Ok(()) => "copiado (OSC 52)".into(),
                    Err(e) => format!("no se pudo copiar: {e}"),
                }
            }
            Err(m) => clean(&m),
        });
    }

    fn enter_mode(&mut self, a: Action) {
        let Some(p) = self.focus_id().and_then(|id| self.panes.get_mut(&id)) else {
            return;
        };
        let busy = p.xfer.is_some() || p.search.is_some() || p.copy.is_some();
        let Some(emu) = p
            .emu
            .as_ref()
            .filter(|_| matches!(p.state, State::Live) && !busy)
        else {
            return;
        };
        if a == Action::Search {
            p.search = Some(Search::new(emu));
        } else {
            p.copy = Some(CopyMode::new(emu));
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
                if p.search.is_some() || p.copy.is_some() || p.xfer.is_some() {
                    return;
                }
                if emu.paste_needs_confirmation(&text) {
                    let n = text.trim_end_matches(['\n', '\r']).lines().count();
                    p.modal = Some(Modal {
                        dialog: Dialog::paste_confirm(n),
                        then: Then::Paste(text),
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
            State::Live if p.xfer.is_some() => {
                if key == Key::Esc {
                    self.cancel_xfer(id).await;
                }
            }
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
            Action::Search | Action::CopyMode => self.enter_mode(a),
            Action::ZmodemSend => self.open_modal(|_| {
                (
                    send_dialog("Zmodem", "Ejecute antes `rz` en el remoto."),
                    Then::ZmodemSend,
                )
            }),
            Action::KermitSend => self.open_modal(|_| {
                (
                    send_dialog("Kermit", "Ejecute antes `kermit -r` en el remoto."),
                    Then::KermitSend,
                )
            }),
            Action::KermitRecv => {
                let dir = clean(&self.paths.downloads_dir.display().to_string());
                self.open_modal(|_| {
                    (
                        Dialog::confirm_yes(
                            "Kermit",
                            &format!("Recibir ficheros por Kermit en {dir}? (ejecute `kermit -s`)"),
                        ),
                        Then::KermitRecv,
                    )
                });
            }
            Action::RunScript => {
                let dir = self.paths.scripts_dir.clone();
                self.open_modal(|_| (script_dialog(&dir), Then::Script));
            }
            _ => {}
        }
    }

    // ---- Zmodem y scripts ----

    /// Abre un diálogo modal en el panel con foco si está vivo y sin otro diálogo.
    fn open_modal(&mut self, make: impl FnOnce(&Pane) -> (Dialog, Then)) {
        let Some(p) = self.focus_id().and_then(|id| self.panes.get_mut(&id)) else {
            return;
        };
        if matches!(p.state, State::Live) && p.modal.is_none() && p.xfer.is_none() {
            let (dialog, then) = make(p);
            p.modal = Some(Modal { dialog, then });
        }
    }

    async fn modal_answer(&mut self, id: PaneId, then: Then, ans: Answer) {
        match (then, ans) {
            (Then::Paste(t), Answer::Confirm(true)) => self.paste_to(id, &t).await,
            (Then::ZmodemRecv, Answer::Confirm(true)) => {
                let dir = self.paths.downloads_dir.clone();
                self.start_xfer(id, FileProtocol::Zmodem, ZKind::Recv(dir));
            }
            (Then::KermitRecv, Answer::Confirm(true)) => {
                let dir = self.paths.downloads_dir.clone();
                self.start_xfer(id, FileProtocol::Kermit, ZKind::Recv(dir));
            }
            (Then::ZmodemSend, Answer::Secret(Some(v))) => {
                let files = split_paths(&v);
                if files.is_empty() {
                    self.zmodem_decline(id).await;
                } else {
                    self.start_xfer(id, FileProtocol::Zmodem, ZKind::Send(files));
                }
            }
            (Then::KermitSend, Answer::Secret(Some(v))) => {
                let files = split_paths(&v);
                if !files.is_empty() {
                    self.start_xfer(id, FileProtocol::Kermit, ZKind::Send(files));
                }
            }
            (Then::ZmodemRecv | Then::ZmodemSend, _) => self.zmodem_decline(id).await,
            (Then::Script, Answer::Secret(Some(v))) => self.run_script(id, v.trim()).await,
            _ => {}
        }
    }

    /// Rechazo de una petición Zmodem del remoto: lo retenido vuelve al emulador, se avisa
    /// al remoto con la cancelación y no se vuelve a preguntar durante un rato.
    async fn zmodem_decline(&mut self, id: PaneId) {
        let Some(p) = self.panes.get_mut(&id) else {
            return;
        };
        let Some(hold) = p.zm_hold.take() else {
            return; // envío manual cancelado: el remoto no había pedido nada
        };
        p.zm_quiet = Some(Instant::now() + ZMODEM_QUIET);
        deliver(p, &hold);
        self.send(id, &CANCEL).await;
    }

    fn start_xfer(&mut self, id: PaneId, proto: FileProtocol, kind: ZKind) {
        let ui = self.tx.clone();
        let Some(p) = self.panes.get_mut(&id).filter(|p| p.xfer.is_none()) else {
            return;
        };
        let Some(writer) = p.out.as_ref().map(|o| o.writer.clone()) else {
            return;
        };
        let (tx, rx) = mpsc::unbounded_channel();
        if let Some(hold) = p.zm_hold.take() {
            let _ = tx.send(hold); // la petición del remoto que motivó la transferencia
        }
        let receiving = matches!(kind, ZKind::Recv(_));
        let dir = self.paths.downloads_dir.clone();
        let mut io = PaneIo::new(rx, writer);
        let task = tokio::spawn(async move {
            let mut prog = |x: Progress| {
                let _ = ui.send(Msg::Xfer(id, XferMsg::Progress(x)));
            };
            let r = match kind {
                ZKind::Recv(dir) => {
                    friki_zmodem::receive_ex(&mut io, proto, &dir, &mut prog, ZMODEM_MAX_BYTES)
                        .await
                        .map(|r| (r.files, r.leftover))
                }
                ZKind::Send(files) => friki_zmodem::send(&mut io, proto, &files, &mut prog)
                    .await
                    .map(|()| (files, Vec::new())),
            };
            let _ = ui.send(Msg::Xfer(id, XferMsg::Done(r, Box::new(io))));
        });
        p.xfer = Some(Xfer {
            tx,
            task,
            receiving,
            name: proto_name(proto),
            progress: None,
            dir,
        });
    }

    async fn cancel_xfer(&mut self, id: PaneId) {
        let Some(x) = self.panes.get_mut(&id).and_then(|p| p.xfer.take()) else {
            return;
        };
        x.task.abort();
        if let Some(p) = self.panes.get_mut(&id) {
            p.zm_quiet = Some(Instant::now() + ZMODEM_QUIET);
        }
        self.send(id, &CANCEL).await;
        // Un fichero a medias queda en disco: se avisa de dónde (no se borra, por si
        // ya existía uno con ese nombre).
        let partial = x
            .progress
            .as_ref()
            .filter(|_| x.receiving)
            .and_then(|pr| Path::new(&pr.file).file_name().map(|n| x.dir.join(n)));
        self.notice = Some(match partial {
            Some(f) => format!(
                "transferencia {} cancelada; fichero parcial: {}",
                x.name,
                clean(&f.display().to_string())
            ),
            None => format!("transferencia {} cancelada", x.name),
        });
    }

    fn xfer_msg(&mut self, id: PaneId, m: XferMsg) {
        let Some(p) = self.panes.get_mut(&id) else {
            return;
        };
        match m {
            XferMsg::Progress(pr) => {
                if let Some(x) = p.xfer.as_mut() {
                    x.progress = Some(pr);
                }
            }
            XferMsg::Done(r, mut io) => {
                let Some(x) = p.xfer.take() else {
                    return; // cancelada antes de llegar
                };
                // Lo que sobró tras el protocolo y lo que llegó entre su fin y este
                // mensaje (x.tx ya no existe: nada más se pierde) vuelve al emulador.
                let mut rest = match &r {
                    Ok((_, left)) => left.clone(),
                    Err(_) => Vec::new(),
                };
                rest.extend(io.drain_rest());
                if !rest.is_empty() {
                    deliver(p, &rest);
                }
                p.modal = Some(match r.map(|(files, _)| files) {
                    Ok(files) => {
                        let mut l = vec![format!(
                            "{} {} fichero(s):",
                            if x.receiving { "Recibidos" } else { "Enviados" },
                            files.len()
                        )];
                        l.extend(files.iter().map(|f| f.display().to_string()));
                        Modal::notice(x.name, &l, false)
                    }
                    Err(e) => Modal::notice(
                        &format!("{}: transferencia fallida", x.name),
                        &[e.to_string()],
                        true,
                    ),
                });
            }
        }
    }

    /// Nombre dentro del directorio de scripts (con `.rhai` si falta) o ruta explícita.
    fn script_path(&self, text: &str) -> PathBuf {
        let p = Path::new(text);
        if p.is_absolute() || text.contains(std::path::MAIN_SEPARATOR) || text.starts_with('.') {
            return p.to_path_buf();
        }
        let c = self.paths.scripts_dir.join(text);
        if c.extension().is_none() {
            c.with_extension("rhai")
        } else {
            c
        }
    }

    fn alert(&mut self, id: PaneId, title: &str, line: &str) {
        if let Some(p) = self.panes.get_mut(&id) {
            p.modal = Some(Modal::notice(title, &[line.to_string()], true));
        }
    }

    async fn run_script(&mut self, id: PaneId, text: &str) {
        if text.is_empty() {
            return;
        }
        let path = self.script_path(text);
        let src = match tokio::fs::read_to_string(&path).await {
            Ok(s) => s,
            Err(e) => {
                let m = format!("{}: {e}", path.display());
                self.alert(id, "Script", &m);
                return;
            }
        };
        let perms = script_perms(&path, &self.paths.scripts_dir);
        let ui = self.tx.clone();
        let Some(p) = self.panes.get_mut(&id).filter(|p| p.script.is_none()) else {
            return;
        };
        let shared = Arc::new(Shared::default());
        shared.set_writer(p.out.as_ref().map(|o| o.writer.clone()));
        if let Some(e) = &p.emu {
            shared.set_screen(&e.screen());
        }
        let req_tx = ui.clone();
        let host = Arc::new(PaneHost::new(
            shared.clone(),
            Arc::new(move |r| req_tx.send(Msg::Script(id, r)).is_ok()),
        ));
        let cancel = CancelToken::new();
        let token = cancel.clone();
        let task = tokio::spawn(async move {
            let r =
                friki_script::run_cancellable(&src, &[], host, &perms, &Limits::default(), &token)
                    .await;
            let _ = ui.send(Msg::ScriptDone(id, r));
        });
        p.script = Some(ScriptRun {
            shared,
            task,
            cancel,
            cancelled: false,
        });
    }

    fn cancel_script(&mut self) {
        if let Some(s) = self
            .focus_id()
            .and_then(|id| self.panes.get_mut(&id))
            .and_then(|p| p.script.as_mut())
        {
            s.cancelled = true;
            s.shared.cancel();
            s.cancel.cancel();
        }
    }

    fn script_done(&mut self, id: PaneId, r: Result<i64, ScriptError>) {
        let Some(p) = self.panes.get_mut(&id) else {
            return;
        };
        let Some(run) = p.script.take() else {
            return;
        };
        match r {
            // Un mensaje del propio script (`MessageBox`) tiene prioridad.
            Ok(0) => {
                self.notice.get_or_insert_with(|| "script terminado".into());
            }
            Ok(c) => self.notice = Some(format!("script terminado con código {c}")),
            Err(_) if run.cancelled => self.notice = Some("script cancelado".into()),
            Err(e) => {
                p.modal = Some(Modal::notice("Error del script", &[e.to_string()], true));
            }
        }
    }

    async fn script_req(&mut self, id: PaneId, r: HostReq) {
        match r {
            HostReq::Message(t) => self.notice = Some(clean(&t)),
            // Termina la sesión del panel, no el panel ni el script (que puede reconectar).
            HostReq::Disconnect => {
                self.end_session(id, Ok(0));
            }
            HostReq::Connect(target, reply) => {
                let sessions = load_sessions(&self.paths.sessions_dir).unwrap_or_default();
                let Some(p) = self.panes.get_mut(&id) else {
                    let _ = reply.send(Err("el panel ya no existe".into()));
                    return;
                };
                if !p.over() {
                    let _ = reply.send(Err("el panel ya está conectado".into()));
                    return;
                }
                let Some(s) = resolve_target(&sessions, target.trim()) else {
                    let _ = reply.send(Err("destino no válido".into()));
                    return;
                };
                p.host = label(&s);
                p.session = s.clone();
                p.state = State::Connecting;
                p.emu = None;
                p.out = None;
                p.exit = None;
                p.outcome = None;
                p.modal = None;
                p.search = None;
                p.copy = None;
                p.pending_connect = Some(reply);
                p.task = Some(spawn_connect(
                    self.tx.clone(),
                    id,
                    s,
                    self.paths.known_hosts.clone(),
                    p.size,
                ));
            }
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
                } else if p.xfer.is_some() {
                    "ZMODEM".into()
                } else if p.search.is_some() {
                    "búsqueda".into()
                } else if let Some(c) = &p.copy {
                    match c.selecting() {
                        Some(_) => "copia (selección)".into(),
                        None => "copia".into(),
                    }
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
                // `!`: un panel de la pestaña espera respuesta a una petición Zmodem.
                let waiting = t
                    .root
                    .leaves()
                    .iter()
                    .any(|id| self.panes.get(id).is_some_and(|p| p.zm_hold.is_some()));
                let mark = if waiting { "!" } else { "" };
                spans.push(Span::styled(format!(" {}:{}{mark} ", i + 1, t.name), style));
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
                let forwards = p.forwards.join(", ");
                let info = StatusInfo {
                    session: &p.session.name,
                    host: &p.host,
                    state: &state,
                    cipher: &p.cipher,
                    forwards: &forwards,
                    legacy: p.legacy,
                    log_warn: p.out.as_ref().and_then(|o| o.log.warn.as_deref()),
                    script: p.script.is_some(),
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
                screen_to_buffer(s, f.buffer_mut(), area, &p.theme);
                if let Some(e) = &p.emu {
                    if let Some(sr) = &p.search {
                        sr.render(f, area, e);
                    }
                    if let Some(cm) = &p.copy {
                        cm.render(f, area, e);
                    }
                }
                if let Some(x) = &p.xfer {
                    note(f, x.line(area.width), rev, area.height.saturating_sub(1));
                }
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
            p.stop_jobs();
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

fn is_ctrl_c(k: &KeyEvent) -> bool {
    k.modifiers
        .contains(crossterm::event::KeyModifiers::CONTROL)
        && k.code == crossterm::event::KeyCode::Char('c')
}

/// Señal Zmodem en `d`, o partida entre el final de `tail` y el principio de `d`.
fn zmodem_start(tail: &[u8], d: &[u8]) -> Option<ZmodemStart> {
    friki_zmodem::detect(d).or_else(|| {
        if tail.is_empty() {
            return None;
        }
        let mut joined = tail.to_vec();
        joined.extend_from_slice(&d[..d.len().min(8)]);
        friki_zmodem::detect(&joined)
    })
}

/// Guarda los últimos bytes del flujo (la señal Zmodem ocupa 6).
fn keep_tail(tail: &mut Vec<u8>, d: &[u8]) {
    const N: usize = 8;
    tail.extend_from_slice(d);
    if tail.len() > N {
        tail.drain(..tail.len() - N);
    }
}

/// Datos del remoto hacia registro, emulador y script del panel.
fn deliver(p: &mut Pane, d: &[u8]) {
    keep_tail(&mut p.zm_tail, d);
    if let Some(o) = p.out.as_mut() {
        o.log.output(d);
    }
    if let Some(e) = p.emu.as_mut() {
        e.process(d);
        if let Some(sc) = &p.script {
            sc.shared.set_screen(&e.screen());
        }
    }
    if let Some(sc) = &p.script {
        sc.shared.push(d);
    }
}

/// El remoto no espera la respuesta del usuario: se cancela la petición (5 CAN), se calla
/// el protocolo un rato y lo retenido vuelve al emulador.
fn abandon_hold(p: &mut Pane) {
    let Some(hold) = p.zm_hold.take() else {
        return;
    };
    p.zm_quiet = Some(Instant::now() + ZMODEM_QUIET);
    if p.modal
        .as_ref()
        .is_some_and(|m| matches!(m.then, Then::ZmodemRecv | Then::ZmodemSend))
    {
        p.modal = None;
    }
    if let Some(w) = p.out.as_ref().map(|o| o.writer.clone()) {
        tokio::spawn(async move {
            let _ = w.write(&CANCEL).await;
        });
    }
    deliver(p, &hold);
}

/// Permisos de un script: ninguno por defecto (sin lectura ni escritura de ficheros) y
/// `connect` solo si el script está en `scripts_dir` y ni él ni el directorio son
/// escribibles por grupo u otros (otro usuario podría haberlo sustituido).
fn script_perms(path: &Path, dir: &Path) -> Permissions {
    let trusted = match (std::fs::canonicalize(path), std::fs::canonicalize(dir)) {
        (Ok(p), Ok(d)) => p.starts_with(&d) && !loose_mode(&p) && !loose_mode(&d),
        _ => false,
    };
    Permissions {
        read: vec![],
        write: vec![],
        connect: trusted,
    }
}

/// Escribible por grupo u otros (no se puede comprobar: se considera que sí).
fn loose_mode(p: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        match std::fs::metadata(p) {
            Ok(m) => m.permissions().mode() & 0o022 != 0,
            Err(_) => true,
        }
    }
    #[cfg(not(unix))]
    {
        let _ = p;
        false
    }
}

fn send_dialog(name: &str, first_line: &str) -> Dialog {
    let mut d = Dialog::text(
        &format!("{name}: enviar ficheros"),
        "Rutas (separadas por ;): ",
        "",
    );
    d.body = vec![first_line.into()];
    d
}

fn proto_name(p: FileProtocol) -> &'static str {
    match p {
        FileProtocol::Xmodem => "Xmodem",
        FileProtocol::Ymodem => "Ymodem",
        FileProtocol::Zmodem => "Zmodem",
        FileProtocol::Kermit => "Kermit",
    }
}

/// Rutas separadas por `;` tal como las escribe el usuario.
fn split_paths(v: &str) -> Vec<PathBuf> {
    v.split(';')
        .map(str::trim)
        .filter(|x| !x.is_empty())
        .map(PathBuf::from)
        .collect()
}

/// Selector de script: lista los `.rhai` de `dir` y pide un nombre o una ruta.
fn script_dialog(dir: &Path) -> Dialog {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".rhai"))
        .collect();
    names.sort();
    let initial = if names.len() == 1 {
        names[0].as_str()
    } else {
        ""
    };
    let mut d = Dialog::text("Ejecutar script", "Script (nombre o ruta): ", initial);
    d.body = vec![format!("Scripts en {}:", clean(&dir.display().to_string()))];
    if names.is_empty() {
        d.body.push("  (ninguno)".into());
    }
    d.body
        .extend(names.iter().take(12).map(|n| format!("  {}", clean(n))));
    d
}

enum ZKind {
    Recv(PathBuf),
    Send(Vec<PathBuf>),
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
    host_out: Box<dyn Write + Send>,
) -> anyhow::Result<i32>
where
    B: Backend,
    B::Error: std::error::Error + Send + Sync + 'static,
    S: Stream<Item = std::io::Result<Event>> + Unpin,
{
    let (tx, mut rx) = mpsc::unbounded_channel::<Msg>();
    let mut app = App::new(tx, paths, exit_with_last, host_out);
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

    use crate::Log;

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
            downloads_dir: d.join("downloads"),
            scripts_dir: d.join("scripts"),
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
        // Windows reintenta el SYN ante un puerto cerrado: el rechazo tarda ~2 s.
        let wait = if cfg!(windows) { 5000 } else { 800 };
        evs.push((wait, key(KeyCode::Char('x'))));
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

    // ---- panel sintético: búsqueda, copia, Zmodem y scripts ----

    struct Rec(std::sync::Mutex<Vec<u8>>);

    impl Rec {
        fn bytes(&self) -> Vec<u8> {
            self.0.lock().expect("rec").clone()
        }
    }

    #[async_trait::async_trait]
    impl StreamWriter for Rec {
        async fn write(&self, data: &[u8]) -> std::io::Result<()> {
            self.0.lock().expect("rec").extend_from_slice(data);
            Ok(())
        }
        async fn resize(&self, _: u16, _: u16) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[derive(Clone, Default)]
    struct HostBuf(Arc<std::sync::Mutex<Vec<u8>>>);

    impl Write for HostBuf {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            self.0.lock().expect("host").extend_from_slice(b);
            Ok(b.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    struct Fixture {
        app: App<'static>,
        rx: mpsc::UnboundedReceiver<Msg>,
        rec: Arc<Rec>,
        host: HostBuf,
    }

    fn fixture(tag: &str) -> Fixture {
        let d = std::env::temp_dir().join(format!("friki-tui-app-{}-{tag}", std::process::id()));
        let paths: &'static Paths = Box::leak(Box::new(Paths {
            known_hosts: d.join("known_hosts"),
            logs_dir: d.join("logs"),
            sessions_dir: d.join("sessions"),
            downloads_dir: d.join("downloads"),
            scripts_dir: d.join("scripts"),
        }));
        std::fs::create_dir_all(&paths.scripts_dir).expect("scripts");
        let (tx, rx) = mpsc::unbounded_channel();
        let host = HostBuf::default();
        let mut app = App::new(tx, paths, false, Box::new(host.clone()));
        app.clip = |t| Ok(Clipboard::Osc52(format!("OSC:{t}")));
        let rec = Arc::new(Rec(std::sync::Mutex::new(vec![])));
        let mut pane = Pane::new(SessionConfig {
            name: "t".into(),
            host: "h".into(),
            ..Default::default()
        });
        pane.state = State::Live;
        pane.emu = Some(Emulator::new(5, 40, 100));
        pane.out = Some(Out {
            writer: rec.clone(),
            log: Log {
                logger: None,
                input: false,
                warn: None,
            },
            dropped: false,
        });
        app.panes.insert(1, pane);
        app.tabs.push(Tab {
            name: "t".into(),
            root: Node::Leaf(1),
            focus: 1,
        });
        app.next_id = 2;
        Fixture { app, rx, rec, host }
    }

    impl Fixture {
        async fn press(&mut self, c: KeyCode) {
            self.app.on_key(KeyEvent::new(c, KeyModifiers::NONE)).await;
        }

        async fn type_str(&mut self, s: &str) {
            for c in s.chars() {
                self.press(KeyCode::Char(c)).await;
            }
        }

        /// Prefijo (`Ctrl+]`) y la tecla de la acción.
        async fn prefixed(&mut self, c: KeyCode, m: KeyModifiers) {
            let pre = KeyEvent::new(KeyCode::Char(']'), KeyModifiers::CONTROL);
            self.app.on_key(pre).await;
            self.app.on_key(KeyEvent::new(c, m)).await;
        }

        /// Salta el bloqueo de 1 s del diálogo abierto por el remoto.
        fn unlock(&mut self) {
            if let Some(m) = self.app.panes.get_mut(&1).and_then(|p| p.modal.as_mut()) {
                m.dialog.unlock();
            }
        }

        fn data(&mut self, d: &[u8]) {
            self.app.stream_event(1, StreamEvent::Data(d.to_vec()));
        }

        fn pane(&self) -> &Pane {
            self.app.panes.get(&1).expect("panel")
        }

        fn screen(&self) -> String {
            let s = self.pane().emu.as_ref().expect("emu").screen();
            s.cells.iter().map(|c| c.text.as_str()).collect()
        }

        /// Procesa mensajes del panel hasta que el script termina.
        async fn until_script_done(&mut self) {
            while self.pane().script.is_some() {
                let m = tokio::time::timeout(Duration::from_secs(10), self.rx.recv())
                    .await
                    .expect("el script no terminó")
                    .expect("canal");
                self.app.on_msg(m).await;
            }
        }
    }

    #[tokio::test]
    async fn search_and_copy_take_the_keyboard_but_output_keeps_flowing() {
        let mut f = fixture("modes");
        f.data(b"hola mundo\r\nadios\r\nmas\r\n");
        f.prefixed(KeyCode::Char('/'), KeyModifiers::NONE).await;
        assert!(f.pane().search.is_some());
        f.type_str("adi").await;
        assert!(f.rec.bytes().is_empty(), "la búsqueda no envía al remoto");
        f.data(b"nuevo\r\n");
        assert!(f.screen().contains("nuevo"));
        f.press(KeyCode::Esc).await;
        assert!(f.pane().search.is_none());
        // Fuera de la búsqueda, la tecla vuelve al remoto.
        f.type_str("a").await;
        assert_eq!(f.rec.bytes(), b"a");

        f.prefixed(KeyCode::Char('['), KeyModifiers::NONE).await;
        assert!(f.pane().copy.is_some());
        for c in "kkkkk0v$".chars() {
            f.press(KeyCode::Char(c)).await;
        }
        f.press(KeyCode::Char('y')).await;
        assert!(f.pane().copy.is_none());
        assert_eq!(f.rec.bytes(), b"a", "la copia no envía al remoto");
        let sent = f.host.0.lock().expect("host").clone();
        assert!(sent.starts_with(b"OSC:hola mundo"), "{sent:?}");
    }

    #[tokio::test]
    async fn zmodem_request_asks_and_decline_cancels() {
        let mut f = fixture("zdecline");
        f.data(b"\r\n**\x18B00000000000000\r\x11");
        let m = f.pane().modal.as_ref().expect("diálogo");
        assert!(m.dialog.body[0].contains("Recibir ficheros por Zmodem en"));
        assert!(m.dialog.body[0].contains("downloads"));
        f.unlock();
        f.press(KeyCode::Esc).await;
        assert!(f.pane().modal.is_none() && f.pane().xfer.is_none());
        assert_eq!(f.rec.bytes(), [0x18; 5]);
        // El remoto repite la petición: no se vuelve a preguntar.
        f.data(b"**\x18B00000000000000\r\x11");
        assert!(f.pane().modal.is_none());
    }

    #[tokio::test]
    async fn zmodem_accept_enters_transfer_mode_and_esc_cancels() {
        let mut f = fixture("zaccept");
        // La señal llega partida entre dos lecturas.
        f.data(b"x**\x18B0");
        f.data(b"0000000000000\r\x11");
        assert!(f.pane().modal.is_some());
        f.unlock();
        f.press(KeyCode::Char('s')).await; // una sola tecla no basta
        f.press(KeyCode::Enter).await;
        assert!(f.pane().xfer.is_none() && f.pane().modal.is_some());
        f.press(KeyCode::Backspace).await;
        f.type_str("yes").await;
        f.press(KeyCode::Enter).await;
        assert!(f.pane().xfer.is_some());
        f.data(b"basura");
        assert!(!f.screen().contains("basura"), "el flujo va a Zmodem");
        f.press(KeyCode::Char('a')).await; // otras teclas no van al remoto
        f.press(KeyCode::Esc).await;
        assert!(f.pane().xfer.is_none());
        let sent = f.rec.bytes();
        assert!(sent.ends_with(&[0x18; 5]), "{sent:?}");
        assert!(!sent.contains(&b'a'));
    }

    #[tokio::test]
    async fn remote_waiting_for_files_and_manual_send_ask_for_paths() {
        let mut f = fixture("zsend");
        f.data(b"**\x18B01000000023be50\r\x11");
        let m = f.pane().modal.as_ref().expect("diálogo");
        assert!(m.dialog.body[0].contains("rz"));
        f.unlock();
        f.press(KeyCode::Esc).await;
        assert_eq!(f.rec.bytes(), [0x18; 5]);
        // Envío manual: sin petición previa del remoto, cancelar no envía nada.
        f.prefixed(KeyCode::Char('z'), KeyModifiers::NONE).await;
        assert!(f.pane().modal.is_some());
        f.press(KeyCode::Esc).await;
        assert_eq!(f.rec.bytes(), [0x18; 5]);
    }

    fn write_script(f: &Fixture, name: &str, src: &str) {
        std::fs::write(f.app.paths.scripts_dir.join(name), src).expect("script");
    }

    #[tokio::test]
    async fn script_sends_waits_and_reports() {
        let mut f = fixture("script-ok");
        write_script(
            &f,
            "t.rhai",
            "crt.Screen.Send(\"echo friki-ok\\n\");\n\
             if !crt.Screen.WaitForString(\"friki-ok\", 5) { crt.Quit(3); }\n\
             crt.Dialog.MessageBox(\"hecho\");",
        );
        f.prefixed(KeyCode::Char('r'), KeyModifiers::NONE).await;
        assert!(f.pane().modal.is_some(), "diálogo de scripts");
        // Un único script: el campo ya viene con su nombre.
        f.press(KeyCode::Enter).await;
        assert!(f.pane().script.is_some());
        f.data(b"echo friki-ok\r\nfriki-ok\r\n");
        f.until_script_done().await;
        assert_eq!(f.rec.bytes(), b"echo friki-ok\n");
        assert!(f.pane().modal.is_none());
        assert_eq!(f.app.notice.as_deref(), Some("hecho"));
    }

    #[tokio::test]
    async fn script_errors_show_a_notice_and_ctrl_c_cancels() {
        let mut f = fixture("script-err");
        write_script(&f, "mal.rhai", "esto no compila (((");
        f.app.run_script(1, "mal").await;
        f.until_script_done().await;
        assert!(f.pane().modal.is_some(), "aviso del error");
        f.press(KeyCode::Enter).await;

        write_script(
            &f,
            "espera.rhai",
            "crt.Screen.WaitForString(\"nunca\", 60);",
        );
        f.app.run_script(1, "espera.rhai").await;
        assert!(f.pane().script.is_some());
        f.prefixed(KeyCode::Char('c'), KeyModifiers::CONTROL).await;
        f.until_script_done().await;
        assert!(f.pane().modal.is_none());
        assert_eq!(f.app.notice.as_deref(), Some("script cancelado"));
        assert!(f.pane().script.is_none(), "estado del panel limpio");

        // Se puede lanzar otro script después de cancelar.
        write_script(&f, "otro.rhai", "crt.Dialog.MessageBox(\"otro\");");
        f.app.run_script(1, "otro").await;
        assert!(f.pane().script.is_some());
        f.until_script_done().await;
        assert_eq!(f.app.notice.as_deref(), Some("otro"));
    }

    #[tokio::test]
    async fn script_outside_the_scripts_dir_cannot_disconnect() {
        let mut f = fixture("script-conn");
        // Sin permiso (script fuera del directorio de scripts): error de permiso.
        let outside = std::env::temp_dir().join(format!("friki-out-{}.rhai", std::process::id()));
        std::fs::write(&outside, "crt.Session.Disconnect();").expect("script");
        f.app.run_script(1, &outside.display().to_string()).await;
        f.until_script_done().await;
        let m = f.pane().modal.as_ref().expect("aviso");
        assert!(
            m.dialog.body.iter().any(|l| l.contains("permiso denegado")),
            "{:?}",
            m.dialog.body
        );
        assert!(!f.pane().over(), "la sesión sigue");
        let _ = std::fs::remove_file(outside);
    }

    #[tokio::test]
    async fn script_disconnect_ends_only_the_session() {
        let mut f = fixture("script-disc");
        f.app.exit_with_last = true;
        f.app.script_req(1, HostReq::Disconnect).await;
        assert!(f.app.panes.contains_key(&1), "el panel sigue abierto");
        assert!(f.pane().over() && f.pane().out.is_none());
        assert!(f.app.done.is_none(), "la aplicación no sale");
    }

    #[cfg(unix)]
    #[test]
    fn script_permissions_are_empty_and_connect_needs_a_private_script() {
        use std::os::unix::fs::PermissionsExt;
        let d = std::env::temp_dir().join(format!("friki-perms-{}", std::process::id()));
        let dir = d.join("scripts");
        std::fs::create_dir_all(&dir).expect("dir");
        let ok = dir.join("ok.rhai");
        std::fs::write(&ok, "").expect("f");
        let mode = |m| std::fs::set_permissions(&ok, std::fs::Permissions::from_mode(m));
        mode(0o644).expect("m");
        let p = script_perms(&ok, &dir);
        assert!(p.read.is_empty() && p.write.is_empty() && p.connect);
        mode(0o666).expect("m");
        assert!(!script_perms(&ok, &dir).connect, "escribible por otros");
        mode(0o664).expect("m");
        assert!(!script_perms(&ok, &dir).connect, "escribible por el grupo");
        let out = d.join("fuera.rhai");
        std::fs::write(&out, "").expect("f");
        assert!(!script_perms(&out, &dir).connect, "fuera de scripts_dir");
        let _ = std::fs::remove_dir_all(d);
    }

    #[tokio::test]
    async fn zmodem_dialog_discards_early_keys() {
        let mut f = fixture("zlock");
        f.data(b"\r\n**\x18B00000000000000\r\x11");
        f.type_str("yes").await;
        f.press(KeyCode::Enter).await;
        assert!(
            f.pane().xfer.is_none(),
            "teclas anteriores al segundo descartadas"
        );
        assert!(f.pane().modal.is_some());
        // Tampoco el envío acepta rutas antes de tiempo.
        let mut g = fixture("zlock-send");
        g.data(b"**\x18B01000000023be50\r\x11");
        g.type_str("/etc/passwd").await;
        g.press(KeyCode::Enter).await;
        assert!(g.pane().xfer.is_none() && g.pane().modal.is_some());
    }

    #[tokio::test]
    async fn zmodem_hold_overflow_cancels_the_request() {
        let mut f = fixture("zhold");
        f.data(b"\r\n**\x18B00000000000000\r\x11");
        assert!(f.pane().zm_hold.is_some() && f.pane().modal.is_some());
        // El remoto no espera la respuesta del usuario y sigue enviando.
        f.data(&vec![b'x'; ZMODEM_HOLD + 1]);
        assert!(f.pane().zm_hold.is_none() && f.pane().modal.is_none());
        assert!(f.pane().zm_quiet.is_some());
        assert!(f.screen().contains('x'), "lo retenido vuelve al emulador");
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(f.rec.bytes(), [0x18; 5]);
    }

    #[tokio::test]
    async fn tab_bar_marks_a_pane_waiting_for_an_answer() {
        let mut f = fixture("ztab");
        f.app.tabs.push(Tab {
            name: "otra".into(),
            root: Node::Leaf(2),
            focus: 2,
        });
        let shown = |f: &Fixture| {
            let mut t = Terminal::new(TestBackend::new(60, 10)).expect("terminal");
            t.draw(|fr| f.app.render(fr)).expect("draw");
            let b = t.backend().buffer().clone();
            (0..60u16)
                .map(|x| b[(x, 0)].symbol().to_string())
                .collect::<String>()
        };
        assert!(!shown(&f).contains('!'));
        f.data(b"\r\n**\x18B00000000000000\r\x11");
        assert!(shown(&f).contains("1:t!"), "{}", shown(&f));
    }

    #[tokio::test]
    async fn finished_transfer_returns_leftover_and_queued_data_to_the_emulator() {
        let mut f = fixture("zleft");
        let (tx, rx) = mpsc::unbounded_channel();
        let io = PaneIo::new(rx, f.rec.clone());
        tx.send(b"cola".to_vec()).expect("tx");
        f.app.panes.get_mut(&1).expect("p").xfer = Some(Xfer {
            tx,
            task: tokio::spawn(async {}),
            receiving: true,
            name: "Zmodem",
            progress: None,
            dir: PathBuf::new(),
        });
        f.app.xfer_msg(
            1,
            XferMsg::Done(Ok((vec![], b"resto-".to_vec())), Box::new(io)),
        );
        assert!(f.pane().xfer.is_none());
        assert!(f.screen().contains("resto-cola"), "{}", f.screen());
    }

    #[tokio::test]
    async fn cancelling_a_transfer_goes_quiet_and_names_the_partial_file() {
        let mut f = fixture("zcancel");
        let (tx, _rx) = mpsc::unbounded_channel();
        f.app.panes.get_mut(&1).expect("p").xfer = Some(Xfer {
            tx,
            task: tokio::spawn(async {}),
            receiving: true,
            name: "Zmodem",
            progress: Some(Progress {
                file: "../a.bin".into(),
                done: 3,
                total: Some(10),
            }),
            dir: PathBuf::from("/dl"),
        });
        f.app.cancel_xfer(1).await;
        let n = f.app.notice.clone().expect("aviso");
        let partial = PathBuf::from("/dl").join("a.bin").display().to_string();
        assert!(n.contains("parcial") && n.contains(&partial), "{n}");
        assert!(f.pane().zm_quiet.is_some());
        assert_eq!(f.rec.bytes(), [0x18; 5]);
    }

    #[tokio::test]
    async fn picker_shows_config_warnings_and_session_errors_together() {
        let mut f = fixture("picker-warn");
        let sd = f.app.paths.sessions_dir.clone();
        std::fs::create_dir_all(&sd).expect("dir");
        std::fs::write(sd.join("mala.toml"), "esto no es toml ===").expect("f");
        f.app.config_warnings = vec!["aviso-uno".into(), "aviso-dos".into()];
        f.app.open_picker(Mode::Tab);
        let Some(Overlay::Picker { picker, .. }) = &f.app.overlay else {
            panic!("selector");
        };
        let e = picker.error.clone().expect("error");
        assert!(
            e.contains("sesiones guardadas") && e.contains("aviso-uno"),
            "{e}"
        );
        assert!(e.contains("aviso-dos"), "{e}");
    }

    #[tokio::test]
    async fn keyboard_interactive_for_shows_the_asking_host() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let p = UiPrompter {
            ui: UiChannel { tx, pane: 1 },
            host: "destino".into(),
        };
        let t = tokio::spawn(async move {
            p.keyboard_interactive_for("salto.example", "", "", &[("Code: ".into(), false)])
                .await
        });
        let Some(Msg::Ask(1, d, reply)) = rx.recv().await else {
            panic!("diálogo");
        };
        assert!(d.title.contains("salto.example") && !d.title.contains("destino"));
        drop(reply);
        assert!(t.await.expect("tarea").is_none());
    }

    struct Full;

    #[async_trait::async_trait]
    impl StreamWriter for Full {
        async fn write(&self, _: &[u8]) -> std::io::Result<()> {
            Err(std::io::ErrorKind::WouldBlock.into())
        }
        async fn resize(&self, _: u16, _: u16) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn would_block_on_write_shows_dropped_input() {
        let mut f = fixture("wouldblock");
        let out = f.app.panes.get_mut(&1).expect("p").out.as_mut().expect("o");
        out.writer = Arc::new(Full);
        f.press(KeyCode::Char('a')).await;
        let p = f.pane();
        assert!(f.app.state_text(p).contains("ENTRADA DESCARTADA"));
        assert!(!p.over(), "no cierra el panel");
    }
}
