//! Terminales y conexiones SSH. Los comandos son envoltorios finos sobre `open_term`, `write_term`...
//! Cada terminal tiene una tarea que posee su flujo: avisa al abrir, bombea la salida y atiende órdenes.

use std::collections::HashSet;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use friki_core::config::{Forward, Protocol, SessionConfig};
use friki_core::paths;
use friki_core::transport::{StreamEvent, StreamReader, StreamWriter};
use friki_session::{Asker, Reply};
use friki_term::SessionLogger;
use tauri::State;
use tauri::ipc::{Channel, InvokeResponseBody};
use tokio::sync::mpsc;
use tokio::time::Instant;
use zeroize::Zeroizing;

use super::hosts::{find_in, validate};
use crate::dto::{
    AppEvent, ConnId, ForwardStatus, HostKeyDecision, KbdPrompt, Prompt, PromptAnswer, TermEvent,
    TermId, TermTarget,
};
use crate::error::{CmdError, ErrorKind};
use crate::state::{AppState, ConnHandle, PROMPT_TIMEOUT, TermCmd, TermHandle};

/// Trozo de salida: se envía al llenarse o a los 16 ms del primer byte pendiente.
const FLUSH_BYTES: usize = 64 * 1024;
const FLUSH_EVERY: Duration = Duration::from_millis(16);
/// Tope por bloque de entrada y por dimensión de la ventana.
const MAX_WRITE: usize = 1 << 20;
const MAX_DIM: u16 = 1000;

/// Secreto a recordar: cuenta del llavero y valor.
type PendingSecret = (String, Zeroizing<String>);

/// `Asker` del escritorio: los prompts viajan a la UI y la respuesta vuelve por `prompt_answer`.
/// Los secretos se buscan antes en el llavero del sistema (cuenta `password:{user}@{host}:{port}` o
/// `passphrase:{ruta}`). Con `remember` quedan pendientes y solo se guardan en `commit`, cuando la
/// conexión ya autenticó; nunca pasan por la UI.
#[derive(Clone)]
pub struct DeskAsker {
    pub state: AppState,
    /// Cuentas ya servidas desde el llavero: si vuelven a pedirse, el valor guardado falló.
    tried: Arc<Mutex<HashSet<String>>>,
    /// Secretos a recordar si la conexión autentica (`cuenta`, valor).
    pending: Arc<Mutex<Vec<PendingSecret>>>,
}

impl DeskAsker {
    pub fn new(state: AppState) -> Self {
        Self {
            state,
            tried: Arc::default(),
            pending: Arc::default(),
        }
    }

    /// La conexión autenticó: guarda los secretos marcados para recordar.
    pub fn commit(&self) {
        let pending = std::mem::take(&mut *self.pending.lock().unwrap_or_else(|e| e.into_inner()));
        for (account, value) in pending {
            // Si el llavero falla, la conexión sigue; solo no se recuerda.
            let _ = friki_core::secrets::set(&account, &value);
        }
    }
}

fn secret_account(p: &friki_session::Prompt) -> Option<String> {
    match p {
        friki_session::Prompt::Password { user, host, port } => {
            Some(format!("password:{user}@{host}:{port}"))
        }
        friki_session::Prompt::Passphrase { key_path } => Some(format!("passphrase:{key_path}")),
        _ => None,
    }
}

#[async_trait::async_trait]
impl Asker for DeskAsker {
    async fn ask(&self, p: friki_session::Prompt) -> Option<Reply> {
        let account = secret_account(&p);
        if let Some(a) = &account {
            let first = self
                .tried
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(a.clone());
            // ponytail: un error del llavero (sin servicio, bloqueado) cuenta como "no guardado".
            if let (true, Ok(Some(v))) = (first, friki_core::secrets::get(a)) {
                return Some(Reply::Secret(Some(Zeroizing::new(v))));
            }
        }
        // `None` (sin suscriptor, rechazo o plazo vencido) lo interpreta friki-session como cancelar.
        match self.state.ask(to_dto(p), PROMPT_TIMEOUT).await? {
            PromptAnswer::HostKey { decision } => Some(Reply::HostKey(match decision {
                HostKeyDecision::Reject => friki_session::HostKeyDecision::Reject,
                HostKeyDecision::AcceptOnce => friki_session::HostKeyDecision::AcceptOnce,
                HostKeyDecision::AcceptAndSave => friki_session::HostKeyDecision::AcceptAndSave,
            })),
            PromptAnswer::Secret { value, remember } => {
                if let (true, Some(a), Some(v)) = (remember, account, &value) {
                    self.pending
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .push((a, v.clone()));
                }
                Some(Reply::Secret(value))
            }
            PromptAnswer::Multi { values } => Some(Reply::Multi(values)),
        }
    }
}

fn to_dto(p: friki_session::Prompt) -> Prompt {
    use friki_session::Prompt as P;
    match p {
        P::HostKeyUnknown {
            host,
            port,
            algorithm,
            fingerprint,
        } => Prompt::HostKeyUnknown {
            host,
            port,
            algorithm,
            fingerprint,
        },
        P::HostKeyChanged {
            host,
            port,
            algorithm,
            fingerprint,
            previous_fingerprint,
        } => Prompt::HostKeyChanged {
            host,
            port,
            algorithm,
            fingerprint,
            previous_fingerprint,
        },
        P::Password { user, host, port } => Prompt::Password { user, host, port },
        P::Passphrase { key_path } => Prompt::Passphrase { key_path },
        P::KbdInteractive {
            host,
            name,
            instructions,
            prompts,
        } => Prompt::KbdInteractive {
            host,
            name,
            instructions,
            prompts: prompts
                .into_iter()
                .map(|k| KbdPrompt {
                    text: k.text,
                    echo: k.echo,
                })
                .collect(),
        },
    }
}

fn check_size((cols, rows): (u16, u16)) -> Result<(), CmdError> {
    if cols == 0 || rows == 0 || cols > MAX_DIM || rows > MAX_DIM {
        return Err(CmdError::invalid("tamaño de terminal no válido"));
    }
    Ok(())
}

/// Config de la sesión a abrir según el destino (`Local` = shell del sistema).
fn resolve(target: TermTarget, dir: &Path) -> Result<SessionConfig, CmdError> {
    match target {
        TermTarget::Saved { name } => find_in(dir, &name),
        TermTarget::Quick { spec } => {
            validate(&spec)?;
            Ok(*spec)
        }
        TermTarget::Local => Ok(SessionConfig {
            name: "local".into(),
            protocol: Protocol::LocalShell,
            ..Default::default()
        }),
    }
}

/// `open_stream` devuelve `anyhow`: si el error viene de friki-ssh se conserva su tipo.
fn open_err(e: anyhow::Error) -> CmdError {
    match e.downcast::<friki_ssh::Error>() {
        Ok(se) => se.into(),
        Err(other) => other.into(),
    }
}

/// Estado de una redirección (`Ok` = escuchando en su dirección real). Las activas se registran en
/// `state.fwds` para que `fwd_list` y `fwd_stop` las vean.
fn fwd_status(
    state: &AppState,
    conn: Option<ConnId>,
    spec: Forward,
    r: Result<friki_session::ForwardHandle, String>,
) -> ForwardStatus {
    match r {
        Ok(h) => {
            let status = ForwardStatus {
                id: h.id,
                spec,
                listen: h.listen.to_string(),
                active: true,
                error: None,
            };
            if let Some(c) = conn {
                state
                    .fwds
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .entry(c)
                    .or_default()
                    .push((status.clone(), h));
            }
            status
        }
        Err(e) => ForwardStatus {
            id: 0,
            spec,
            listen: String::new(),
            active: false,
            error: Some(e),
        },
    }
}

/// `false` si la UI ya no escucha.
fn flush(output: &Channel<InvokeResponseBody>, buf: &mut Vec<u8>) -> bool {
    if buf.is_empty() {
        return true;
    }
    output
        .send(InvokeResponseBody::Raw(std::mem::take(buf)))
        .is_ok()
}

/// Bombea la salida (agrupada) y las órdenes hasta que termina el flujo o se cierra. Devuelve el evento final.
async fn pump(
    writer: &dyn StreamWriter,
    reader: &mut dyn StreamReader,
    rx: &mut mpsc::UnboundedReceiver<TermCmd>,
    output: &Channel<InvokeResponseBody>,
    logger: &mut Option<Box<SessionLogger>>,
) -> TermEvent {
    let mut buf = Vec::new();
    let mut due: Option<Instant> = None;
    let end = loop {
        let timer = tokio::time::sleep_until(due.unwrap_or_else(Instant::now));
        tokio::select! {
            cmd = rx.recv() => match cmd {
                Some(TermCmd::Write(d)) => {
                    // Un fallo de disco apaga el registro, no la sesión.
                    if logger.as_mut().is_some_and(|l| l.log_input(&d).is_err()) {
                        *logger = None;
                    }
                    if let Err(e) = writer.write(&d).await {
                        break TermEvent::Lost { reason: Some(e.to_string()) };
                    }
                }
                Some(TermCmd::Resize(c, r)) => {
                    let _ = writer.resize(c, r).await;
                }
                Some(TermCmd::Log(l)) => *logger = l,
                Some(TermCmd::Close) | None => break TermEvent::Closed,
            },
            ev = reader.next_event() => match ev {
                Some(StreamEvent::Data(d)) => {
                    if logger.as_mut().is_some_and(|l| l.log_output(&d).is_err()) {
                        *logger = None;
                    }
                    if buf.is_empty() {
                        due = Some(Instant::now() + FLUSH_EVERY);
                    }
                    buf.extend_from_slice(&d);
                    if buf.len() >= FLUSH_BYTES {
                        if !flush(output, &mut buf) {
                            break TermEvent::Closed;
                        }
                        due = None;
                    }
                }
                Some(StreamEvent::Exit(code)) => break TermEvent::Exit { code: Some(code) },
                Some(StreamEvent::Closed) | None => break TermEvent::Closed,
                Some(StreamEvent::Lost(reason)) => break TermEvent::Lost { reason: Some(reason) },
            },
            _ = timer, if due.is_some() => {
                if !flush(output, &mut buf) {
                    break TermEvent::Closed;
                }
                due = None;
            }
        }
    };
    // Lo pendiente sale antes del evento final.
    flush(output, &mut buf);
    end
}

/// Tarea de una terminal: conecta, avisa y bombea E/S. Al acabar se da de baja del registro.
async fn run_term(
    state: AppState,
    id: TermId,
    cfg: SessionConfig,
    size: (u16, u16),
    mut rx: mpsc::UnboundedReceiver<TermCmd>,
    output: Channel<InvokeResponseBody>,
    events: Channel<TermEvent>,
) {
    let asker = DeskAsker::new(state.clone());
    let open = friki_session::open_stream(asker.clone(), &cfg, paths::known_hosts_file(), size);
    tokio::pin!(open);
    // Mientras conecta: las escrituras se guardan para enviarlas al abrir; cerrar cancela.
    let mut early: Vec<Vec<u8>> = Vec::new();
    let mut latest: Option<(u16, u16)> = None;
    let mut logger: Option<Box<SessionLogger>> = None;
    let res = loop {
        tokio::select! {
            r = &mut open => break r,
            cmd = rx.recv() => match cmd {
                Some(TermCmd::Write(d)) => early.push(d),
                Some(TermCmd::Resize(c, r)) => latest = Some((c, r)),
                Some(TermCmd::Log(l)) => logger = l,
                Some(TermCmd::Close) | None => {
                    state.terms.remove(id);
                    return;
                }
            },
        }
    };
    let (opened, mut reader) = match res {
        Ok(v) => v,
        Err(e) => {
            state.terms.remove(id);
            let _ = events.send(TermEvent::Failed { error: open_err(e) });
            return;
        }
    };
    // Conectado y autenticado: solo ahora se guardan los secretos marcados para recordar.
    asker.commit();
    let friki_session::Opened {
        conn,
        writer,
        warnings,
        negotiated,
        forwards,
        ..
    } = opened;
    let conn_id = conn
        .as_ref()
        .map(|c| state.conns.insert(ConnHandle { conn: c.clone() }));
    let _ = events.send(TermEvent::Opened {
        conn: conn_id,
        warnings,
        negotiated: negotiated.map(|n| format!("{} {} {} {}", n.kex, n.cipher, n.mac, n.host_key)),
        forwards: forwards
            .into_iter()
            .map(|(spec, r)| fwd_status(&state, conn_id, spec, r))
            .collect(),
    });
    if let Some((c, r)) = latest {
        let _ = writer.resize(c, r).await;
    }
    for d in early {
        let _ = writer.write(&d).await;
    }
    let end = pump(&*writer, &mut *reader, &mut rx, &output, &mut logger).await;
    state.terms.remove(id);
    drop((writer, reader));
    if let Some(c) = conn_id {
        // La terminal posee su conexión: al terminar se cierra con sus redirecciones. Si el
        // transporte murió, también; si no, SFTP abierto sobre ella la mantiene viva hasta `conn_close`.
        let (lost, reason) = match &end {
            TermEvent::Lost { reason } => (true, reason.clone()),
            _ => (false, None),
        };
        if lost || !state.sftps.any(|s| s.conn == Some(c)) {
            close_conn(&state, c, reason).await;
        }
    }
    let _ = events.send(end);
}

#[tauri::command]
pub async fn term_open(
    state: State<'_, AppState>,
    target: TermTarget,
    cols: u16,
    rows: u16,
    output: Channel<InvokeResponseBody>,
    events: Channel<TermEvent>,
) -> Result<TermId, CmdError> {
    open_term(
        &state,
        &paths::sessions_dir(),
        target,
        (cols, rows),
        output,
        events,
    )
}

/// Reserva el id y lanza la tarea; vuelve antes de conectar.
pub fn open_term(
    state: &AppState,
    dir: &Path,
    target: TermTarget,
    size: (u16, u16),
    output: Channel<InvokeResponseBody>,
    events: Channel<TermEvent>,
) -> Result<TermId, CmdError> {
    check_size(size)?;
    let cfg = resolve(target, dir)?;
    let (tx, rx) = mpsc::unbounded_channel();
    let name = if cfg.name.is_empty() {
        &cfg.host
    } else {
        &cfg.name
    }
    .clone();
    let host = cfg.host.clone();
    let id = state.terms.insert(TermHandle { tx, name, host });
    tokio::spawn(run_term(state.clone(), id, cfg, size, rx, output, events));
    Ok(id)
}

#[tauri::command]
pub async fn term_write(
    state: State<'_, AppState>,
    id: TermId,
    data: Vec<u8>,
) -> Result<(), CmdError> {
    write_term(&state, id, data)
}

pub fn write_term(state: &AppState, id: TermId, data: Vec<u8>) -> Result<(), CmdError> {
    if data.len() > MAX_WRITE {
        return Err(CmdError::invalid("bloque de entrada demasiado grande"));
    }
    send(state, id, TermCmd::Write(data))
}

#[tauri::command]
pub async fn term_resize(
    state: State<'_, AppState>,
    id: TermId,
    cols: u16,
    rows: u16,
) -> Result<(), CmdError> {
    check_size((cols, rows))?;
    send(&state, id, TermCmd::Resize(cols, rows))
}

#[tauri::command]
pub async fn term_close(state: State<'_, AppState>, id: TermId) -> Result<(), CmdError> {
    close_term(&state, id);
    Ok(())
}

/// Idempotente: cerrar un id desconocido no es error.
pub fn close_term(state: &AppState, id: TermId) {
    if let Some(h) = state.terms.remove(id) {
        let _ = h.tx.send(TermCmd::Close);
    }
}

/// Empieza a registrar la terminal en `dir` con la plantilla por defecto (`%S-%D.log`, solo salida:
/// la entrada tras un aviso de contraseña nunca se escribiría, y aquí ni siquiera se registra).
/// Devuelve la ruta del fichero; `SessionLogger` rechaza cualquier ruta fuera de `dir`.
pub fn start_log(state: &AppState, dir: &Path, id: TermId) -> Result<String, CmdError> {
    let (name, host) = state
        .terms
        .with(id, |h| (h.name.clone(), h.host.clone()))
        .ok_or_else(|| CmdError::invalid("terminal desconocido o cerrado"))?;
    let logger = SessionLogger::open(&friki_core::config::LogConfig::default(), dir, &name, &host)
        .map_err(|e| {
            CmdError::new(
                ErrorKind::Other,
                format!("no se pudo abrir el registro: {e}"),
            )
        })?;
    let path = logger.path().to_string_lossy().into_owned();
    send(state, id, TermCmd::Log(Some(Box::new(logger))))?;
    Ok(path)
}

#[tauri::command]
pub async fn term_log_start(state: State<'_, AppState>, id: TermId) -> Result<String, CmdError> {
    start_log(&state, &paths::logs_dir(), id)
}

#[tauri::command]
pub async fn term_log_stop(state: State<'_, AppState>, id: TermId) -> Result<(), CmdError> {
    send(&state, id, TermCmd::Log(None))
}

/// Solo rutas de ficheros existentes dentro de `logs` (tras resolver `..` y enlaces simbólicos).
fn check_reveal(path: &str, logs: &Path) -> Result<std::path::PathBuf, CmdError> {
    let bad = || CmdError::invalid("solo se pueden mostrar ficheros del directorio de registros");
    let p = Path::new(path).canonicalize().map_err(|_| bad())?;
    let base = logs.canonicalize().map_err(|_| bad())?;
    if p.starts_with(&base) {
        Ok(p)
    } else {
        Err(bad())
    }
}

#[tauri::command]
pub async fn reveal_path(path: String) -> Result<(), CmdError> {
    let p = check_reveal(&path, &paths::logs_dir())?;
    #[cfg(target_os = "macos")]
    let mut cmd = {
        let mut c = std::process::Command::new("open");
        c.arg("-R").arg(&p);
        c
    };
    #[cfg(target_os = "windows")]
    let mut cmd = {
        let mut c = std::process::Command::new("explorer");
        c.arg(format!("/select,{}", p.display()));
        c
    };
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let mut cmd = {
        let mut c = std::process::Command::new("xdg-open");
        c.arg(p.parent().unwrap_or(&p));
        c
    };
    cmd.spawn()
        .map(|mut c| {
            std::thread::spawn(move || {
                let _ = c.wait();
            });
        })
        .map_err(|e| {
            CmdError::new(
                ErrorKind::Other,
                format!("no se pudo abrir el gestor de archivos: {e}"),
            )
        })
}

fn send(state: &AppState, id: TermId, cmd: TermCmd) -> Result<(), CmdError> {
    match state.terms.with(id, |h| h.tx.send(cmd).is_ok()) {
        Some(true) => Ok(()),
        _ => Err(CmdError::invalid("terminal desconocido o cerrado")),
    }
}

#[tauri::command]
pub async fn conn_open(state: State<'_, AppState>, host: String) -> Result<ConnId, CmdError> {
    open_conn(&state, &paths::sessions_dir(), &host).await
}

/// SSH a un host guardado. Sus redirecciones se activan al conectar (estado por `AppEvent::Forward`).
pub async fn open_conn(state: &AppState, dir: &Path, host: &str) -> Result<ConnId, CmdError> {
    let cfg = find_in(dir, host)?;
    if cfg.protocol != Protocol::Ssh {
        return Err(CmdError::invalid("solo se puede conectar por SSH"));
    }
    let asker = DeskAsker::new(state.clone());
    let conn =
        Arc::new(friki_session::connect_ssh(asker.clone(), &cfg, paths::known_hosts_file()).await?);
    asker.commit();
    let id = state.conns.insert(ConnHandle { conn: conn.clone() });
    for f in &cfg.forwards {
        let r = conn.start_forward(f).await.map_err(|e| e.to_string());
        state.emit(AppEvent::Forward {
            conn: id,
            status: fwd_status(state, Some(id), f.clone(), r),
        });
    }
    Ok(id)
}

#[tauri::command]
pub async fn conn_close(state: State<'_, AppState>, conn: ConnId) -> Result<(), CmdError> {
    close_conn(&state, conn, None).await;
    Ok(())
}

/// Quita la conexión del registro con sus redirecciones, sus sesiones SFTP y sus colas; avisa a la UI
/// y cierra la sesión SSH. Cierra aunque otra tarea conserve una referencia (`Arc`): no depende de
/// que sea la última, así que nunca se queda abierta.
pub async fn close_conn(state: &AppState, conn: ConnId, reason: Option<String>) {
    let Some(h) = state.conns.remove(conn) else {
        return;
    };
    state
        .fwds
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&conn);
    let sftps = state.sftps.remove_where(|s| s.conn == Some(conn));
    state
        .queues
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .retain(|(sftp, _), _| !sftps.contains(sftp));
    state.emit(AppEvent::ConnClosed { conn, reason });
    let _ = h.conn.close().await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::ErrorKind;
    use tauri::ipc::Channel;

    /// Canal de pruebas: cada mensaje que envía el backend llega a un `mpsc`.
    fn sink<T>() -> (Channel<T>, mpsc::UnboundedReceiver<InvokeResponseBody>) {
        let (tx, rx) = mpsc::unbounded_channel();
        let ch = Channel::new(move |body| {
            let _ = tx.send(body);
            Ok(())
        });
        (ch, rx)
    }

    fn find_u64(v: &serde_json::Value, key: &str) -> Option<u64> {
        match v {
            serde_json::Value::Object(m) => m
                .get(key)
                .and_then(serde_json::Value::as_u64)
                .or_else(|| m.values().find_map(|x| find_u64(x, key))),
            serde_json::Value::Array(a) => a.iter().find_map(|x| find_u64(x, key)),
            _ => None,
        }
    }

    #[tokio::test]
    async fn asker_ida_y_vuelta_por_el_registro() {
        let state = AppState::default();
        let (events, mut ui) = sink::<AppEvent>();
        state.subscribe(events);
        let asker = DeskAsker::new(state.clone());
        let ask = tokio::spawn(async move {
            asker
                .ask(friki_session::Prompt::Password {
                    user: "u".into(),
                    host: "h".into(),
                    port: 22,
                })
                .await
        });
        let InvokeResponseBody::Json(json) = ui.recv().await.unwrap() else {
            panic!("el evento debe ir como JSON");
        };
        let id = find_u64(&serde_json::from_str(&json).unwrap(), "id").unwrap();
        let answer =
            serde_json::from_str(r#"{"kind":"secret","value":"pw","remember":false}"#).unwrap();
        state.prompts.answer(id, answer).unwrap();
        match ask.await.unwrap() {
            Some(Reply::Secret(Some(v))) => assert_eq!(v.as_str(), "pw"),
            _ => panic!("respuesta inesperada"),
        }
    }

    #[tokio::test]
    async fn secreto_recordado_queda_pendiente_hasta_autenticar() {
        let state = AppState::default();
        let (events, mut ui) = sink::<AppEvent>();
        state.subscribe(events);
        let asker = DeskAsker::new(state.clone());
        let a2 = asker.clone();
        let ask = tokio::spawn(async move {
            a2.ask(friki_session::Prompt::Password {
                user: "u".into(),
                host: "h".into(),
                port: 2222,
            })
            .await
        });
        let InvokeResponseBody::Json(json) = ui.recv().await.unwrap() else {
            panic!("el evento debe ir como JSON");
        };
        let id = find_u64(&serde_json::from_str(&json).unwrap(), "id").unwrap();
        let answer =
            serde_json::from_str(r#"{"kind":"secret","value":"mal","remember":true}"#).unwrap();
        state.prompts.answer(id, answer).unwrap();
        assert!(ask.await.unwrap().is_some());
        // Sin `commit` (la autenticación no llegó) nada se guarda; la cuenta usa el puerto del prompt.
        let pending = asker.pending.lock().unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].0, "password:u@h:2222");
    }

    #[tokio::test]
    async fn sin_suscriptor_el_prompt_es_rechazo() {
        let asker = DeskAsker::new(AppState::default());
        let r = asker
            .ask(friki_session::Prompt::Passphrase {
                key_path: "/k".into(),
            })
            .await;
        assert!(r.is_none());
    }

    #[tokio::test]
    async fn tamanos_y_ids_invalidos() {
        let state = AppState::default();
        let dir = tempfile::tempdir().unwrap();
        let (out, _o) = sink::<InvokeResponseBody>();
        let (ev, _e) = sink::<TermEvent>();
        let e = open_term(&state, dir.path(), TermTarget::Local, (0, 24), out, ev).unwrap_err();
        assert_eq!(e.kind, ErrorKind::Invalid);
        assert_eq!(
            write_term(&state, 42, vec![1]).unwrap_err().kind,
            ErrorKind::Invalid
        );
        close_term(&state, 42); // idempotente
    }

    #[tokio::test]
    async fn local_end_to_end_por_friki_pty() {
        let state = AppState::default();
        let dir = tempfile::tempdir().unwrap();
        let (out, mut out_rx) = sink::<InvokeResponseBody>();
        let (ev, mut ev_rx) = sink::<TermEvent>();
        let id = open_term(&state, dir.path(), TermTarget::Local, (80, 24), out, ev).unwrap();
        // La escritura puede llegar antes de abrir: se guarda y se envía luego.
        write_term(&state, id, b"echo $((6*7))\nexit\n".to_vec()).unwrap();

        let last = tokio::time::timeout(Duration::from_secs(20), async {
            loop {
                let InvokeResponseBody::Json(j) = ev_rx.recv().await.expect("sin eventos") else {
                    panic!("evento como JSON");
                };
                let v: serde_json::Value = serde_json::from_str(&j).unwrap();
                match v["type"].as_str() {
                    Some("opened") => {}
                    Some(t) => return t.to_string(),
                    None => panic!("evento sin tipo: {j}"),
                }
            }
        })
        .await
        .expect("plazo vencido");
        assert!(last == "exit" || last == "closed", "fin: {last}");

        // La salida va antes del evento final.
        let mut bytes = Vec::new();
        while let Ok(InvokeResponseBody::Raw(b)) = out_rx.try_recv() {
            bytes.extend(b);
        }
        assert!(String::from_utf8_lossy(&bytes).contains("42"), "{bytes:?}");
    }

    fn fake_term(state: &AppState, name: &str) -> (TermId, mpsc::UnboundedReceiver<TermCmd>) {
        let (tx, rx) = mpsc::unbounded_channel();
        let id = state.terms.insert(TermHandle {
            tx,
            name: name.into(),
            host: "h".into(),
        });
        (id, rx)
    }

    #[test]
    fn log_start_stays_inside_logs_dir() {
        let d = tempfile::tempdir().unwrap();
        let state = AppState::default();
        // Un nombre malicioso no puede escapar del directorio de registros.
        let (id, mut rx) = fake_term(&state, "../../evil\x1b");
        let path = start_log(&state, d.path(), id).unwrap();
        let p = std::path::PathBuf::from(&path);
        assert!(p.starts_with(d.path()), "{path}");
        assert!(p.exists());
        assert!(matches!(rx.try_recv(), Ok(TermCmd::Log(Some(_)))));
        // check_reveal acepta justo esa ruta.
        assert!(check_reveal(&path, d.path()).is_ok());
    }

    #[test]
    fn log_start_unknown_term_and_stop() {
        let d = tempfile::tempdir().unwrap();
        let state = AppState::default();
        assert_eq!(
            start_log(&state, d.path(), 99).unwrap_err().kind,
            ErrorKind::Invalid
        );
        let (id, mut rx) = fake_term(&state, "s");
        send(&state, id, TermCmd::Log(None)).unwrap();
        assert!(matches!(rx.try_recv(), Ok(TermCmd::Log(None))));
    }

    #[test]
    fn reveal_rejects_paths_outside_logs() {
        let logs = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        let outside = other.path().join("x.log");
        std::fs::write(&outside, "x").unwrap();
        let inside = logs.path().join("a.log");
        std::fs::write(&inside, "x").unwrap();
        let rej = |p: &str| check_reveal(p, logs.path()).unwrap_err().kind == ErrorKind::Invalid;
        assert!(rej(outside.to_str().unwrap()));
        assert!(rej(logs.path().join("../nope.log").to_str().unwrap()));
        assert!(rej(logs.path().join("missing.log").to_str().unwrap()));
        assert!(rej("/etc/hosts"));
        assert!(check_reveal(inside.to_str().unwrap(), logs.path()).is_ok());
        #[cfg(unix)]
        {
            let link = logs.path().join("l.log");
            std::os::unix::fs::symlink(&outside, &link).unwrap();
            assert!(rej(link.to_str().unwrap()), "enlace que escapa");
        }
    }
}
