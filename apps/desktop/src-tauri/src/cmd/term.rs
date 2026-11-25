//! Terminales y conexiones SSH. Los comandos son envoltorios finos sobre `open_term`, `write_term`...
//! Cada terminal tiene una tarea que posee su flujo: avisa al abrir, bombea la salida y atiende órdenes.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use friki_core::config::{Forward, Protocol, SessionConfig};
use friki_core::paths;
use friki_core::transport::{StreamEvent, StreamReader, StreamWriter};
use friki_session::{Asker, Reply};
use tauri::State;
use tauri::ipc::{Channel, InvokeResponseBody};
use tokio::sync::mpsc;
use tokio::time::Instant;

use super::hosts::{find_in, validate};
use crate::dto::{
    AppEvent, ConnId, ForwardStatus, HostKeyDecision, KbdPrompt, Prompt, PromptAnswer, TermEvent,
    TermId, TermTarget,
};
use crate::error::CmdError;
use crate::state::{AppState, ConnHandle, PROMPT_TIMEOUT, TermCmd, TermHandle};

/// Trozo de salida: se envía al llenarse o a los 16 ms del primer byte pendiente.
const FLUSH_BYTES: usize = 64 * 1024;
const FLUSH_EVERY: Duration = Duration::from_millis(16);
/// Tope por bloque de entrada y por dimensión de la ventana.
const MAX_WRITE: usize = 1 << 20;
const MAX_DIM: u16 = 1000;

/// `Asker` del escritorio: los prompts viajan a la UI y la respuesta vuelve por `prompt_answer`.
#[derive(Clone)]
pub struct DeskAsker(pub AppState);

#[async_trait::async_trait]
impl Asker for DeskAsker {
    async fn ask(&self, p: friki_session::Prompt) -> Option<Reply> {
        // `None` (sin suscriptor, rechazo o plazo vencido) lo interpreta friki-session como cancelar.
        match self.0.ask(to_dto(p), PROMPT_TIMEOUT).await? {
            PromptAnswer::HostKey { decision } => Some(Reply::HostKey(match decision {
                HostKeyDecision::Reject => friki_session::HostKeyDecision::Reject,
                HostKeyDecision::AcceptOnce => friki_session::HostKeyDecision::AcceptOnce,
                HostKeyDecision::AcceptAndSave => friki_session::HostKeyDecision::AcceptAndSave,
            })),
            // ponytail: `remember` se ignora; el llavero (T3) define la cuenta y se cablea allí.
            PromptAnswer::Secret { value, remember: _ } => Some(Reply::Secret(value)),
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
        P::Password { user, host } => Prompt::Password { user, host },
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

/// Estado de una redirección (`Ok` = escuchando en su dirección real).
fn fwd_status(spec: Forward, r: Result<friki_session::ForwardHandle, String>) -> ForwardStatus {
    match r {
        Ok(h) => ForwardStatus {
            id: h.id,
            spec,
            listen: h.listen.to_string(),
            active: true,
            error: None,
        },
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
) -> TermEvent {
    let mut buf = Vec::new();
    let mut due: Option<Instant> = None;
    let end = loop {
        let timer = tokio::time::sleep_until(due.unwrap_or_else(Instant::now));
        tokio::select! {
            cmd = rx.recv() => match cmd {
                Some(TermCmd::Write(d)) => {
                    if let Err(e) = writer.write(&d).await {
                        break TermEvent::Lost { reason: Some(e.to_string()) };
                    }
                }
                Some(TermCmd::Resize(c, r)) => {
                    let _ = writer.resize(c, r).await;
                }
                Some(TermCmd::Close) | None => break TermEvent::Closed,
            },
            ev = reader.next_event() => match ev {
                Some(StreamEvent::Data(d)) => {
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
    let open = friki_session::open_stream(
        DeskAsker(state.clone()),
        &cfg,
        paths::known_hosts_file(),
        size,
    );
    tokio::pin!(open);
    // Mientras conecta: las escrituras se guardan para enviarlas al abrir; cerrar cancela.
    let mut early: Vec<Vec<u8>> = Vec::new();
    let mut latest: Option<(u16, u16)> = None;
    let res = loop {
        tokio::select! {
            r = &mut open => break r,
            cmd = rx.recv() => match cmd {
                Some(TermCmd::Write(d)) => early.push(d),
                Some(TermCmd::Resize(c, r)) => latest = Some((c, r)),
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
            .map(|(spec, r)| fwd_status(spec, r))
            .collect(),
    });
    if let Some((c, r)) = latest {
        let _ = writer.resize(c, r).await;
    }
    for d in early {
        let _ = writer.write(&d).await;
    }
    let end = pump(&*writer, &mut *reader, &mut rx, &output).await;
    state.terms.remove(id);
    let _ = events.send(end);
    drop((writer, reader));
    // Si la conexión sigue en el registro (ConnId), no se cierra aquí.
    if let Some(c) = conn {
        friki_session::disconnect(c).await;
    }
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
    let id = state.terms.insert(TermHandle { tx });
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
    let conn = Arc::new(
        friki_session::connect_ssh(DeskAsker(state.clone()), &cfg, paths::known_hosts_file())
            .await?,
    );
    let id = state.conns.insert(ConnHandle { conn: conn.clone() });
    for f in &cfg.forwards {
        let r = conn.start_forward(f).await.map_err(|e| e.to_string());
        state.emit(AppEvent::Forward {
            conn: id,
            status: fwd_status(f.clone(), r),
        });
    }
    Ok(id)
}

#[tauri::command]
pub async fn conn_close(state: State<'_, AppState>, conn: ConnId) -> Result<(), CmdError> {
    close_conn(&state, conn).await;
    Ok(())
}

/// Quita la conexión del registro; se desconecta si ya no la usa nadie.
pub async fn close_conn(state: &AppState, conn: ConnId) {
    if let Some(h) = state.conns.remove(conn) {
        friki_session::disconnect(h.conn).await;
    }
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
        let asker = DeskAsker(state.clone());
        let ask = tokio::spawn(async move {
            asker
                .ask(friki_session::Prompt::Password {
                    user: "u".into(),
                    host: "h".into(),
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
    async fn sin_suscriptor_el_prompt_es_rechazo() {
        let asker = DeskAsker(AppState::default());
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
}
