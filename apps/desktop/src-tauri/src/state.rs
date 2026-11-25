//! Estado de la app: registros de recursos vivos, ids y registro de prompts.

use std::collections::HashMap;
use std::hash::{BuildHasher, RandomState};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use friki_ssh::{Connection, ForwardHandle};
use friki_xfer::RemoteFs;
use friki_xfer::queue::Queue;
use tauri::ipc::Channel;
use tokio::sync::{mpsc, oneshot};

use crate::dto::{
    AppEvent, ConnId, ForwardStatus, Prompt, PromptAnswer, PromptId, PromptKind, SftpId,
};
use crate::error::{CmdError, ErrorKind};

/// Plazo para contestar un prompt; vencido cuenta como rechazo.
pub const PROMPT_TIMEOUT: Duration = Duration::from_secs(120);

/// Los id llegan a JS como `number`: máximo 2^53 - 1.
const JS_SAFE_MASK: u64 = (1 << 53) - 1;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Orden a la tarea de una terminal (la que posee el flujo).
pub enum TermCmd {
    Write(Vec<u8>),
    Resize(u16, u16),
    Close,
}

pub struct TermHandle {
    pub tx: mpsc::UnboundedSender<TermCmd>,
}

/// Conexión SSH compartida por terminal, SFTP y redirecciones.
pub struct ConnHandle {
    pub conn: Arc<Connection>,
}

/// Redirecciones vivas por conexión.
pub type ForwardMap = HashMap<ConnId, Vec<(ForwardStatus, ForwardHandle)>>;

/// Sistema de ficheros remoto abierto con `sftp_open`.
pub struct SftpHandle {
    pub fs: Arc<dyn RemoteFs>,
    /// Pausa de la sesión: las colas nuevas nacen con ella.
    pub paused: AtomicBool,
    /// Conexión de origen: las colas la usan para reabrir SFTP tras un corte.
    pub conn: Option<ConnId>,
}

/// Cola de transferencias de un (sftp, raíz local). `seq` va en los `JobId`.
/// `pending` cuenta trabajos sin evento final: al llegar a 0 la cola se libera.
pub struct QueueHandle {
    pub seq: u64,
    pub queue: Queue,
    pub pending: usize,
}

/// Mapa id -> recurso con ids secuenciales.
pub struct Registry<T> {
    next: AtomicU64,
    map: Mutex<HashMap<u64, T>>,
}

impl<T> Default for Registry<T> {
    fn default() -> Self {
        Self {
            next: AtomicU64::new(1),
            map: Mutex::default(),
        }
    }
}

impl<T> Registry<T> {
    pub fn insert(&self, v: T) -> u64 {
        let id = self.next.fetch_add(1, Relaxed);
        lock(&self.map).insert(id, v);
        id
    }
    pub fn remove(&self, id: u64) -> Option<T> {
        lock(&self.map).remove(&id)
    }
    pub fn contains(&self, id: u64) -> bool {
        lock(&self.map).contains_key(&id)
    }
    /// Usa el recurso sin sacarlo del mapa.
    pub fn with<R>(&self, id: u64, f: impl FnOnce(&T) -> R) -> Option<R> {
        lock(&self.map).get(&id).map(f)
    }
    pub fn any(&self, f: impl Fn(&T) -> bool) -> bool {
        lock(&self.map).values().any(f)
    }
    /// Quita los recursos que cumplen `f` y devuelve sus ids.
    pub fn remove_where(&self, f: impl Fn(&T) -> bool) -> Vec<u64> {
        let mut map = lock(&self.map);
        let ids: Vec<u64> = map.iter().filter(|(_, v)| f(v)).map(|(k, _)| *k).collect();
        for id in &ids {
            map.remove(id);
        }
        ids
    }
}

/// `PromptId -> (tipo esperado, oneshot)`. Ids aleatorios y de un solo uso.
#[derive(Default)]
pub struct Prompts {
    pending: Mutex<HashMap<PromptId, (PromptKind, oneshot::Sender<PromptAnswer>)>>,
    seq: AtomicU64,
}

impl Prompts {
    pub fn register(&self, kind: PromptKind) -> (PromptId, oneshot::Receiver<PromptAnswer>) {
        let (tx, rx) = oneshot::channel();
        let mut pending = lock(&self.pending);
        let id = loop {
            // ponytail: RandomState (semilla aleatoria por proceso) en vez de añadir `rand`.
            let id = RandomState::new().hash_one(self.seq.fetch_add(1, Relaxed)) & JS_SAFE_MASK;
            if id != 0 && !pending.contains_key(&id) {
                break id;
            }
        };
        pending.insert(id, (kind, tx));
        (id, rx)
    }

    /// Id desconocido, ya contestado o de otro tipo: `invalid`. Un tipo equivocado no consume el prompt.
    pub fn answer(&self, id: PromptId, answer: PromptAnswer) -> Result<(), CmdError> {
        let mut pending = lock(&self.pending);
        match pending.get(&id) {
            None => return Err(CmdError::invalid("prompt desconocido o ya contestado")),
            Some((kind, _)) if *kind != answer.kind() => {
                return Err(CmdError::invalid("la respuesta no corresponde al prompt"));
            }
            Some(_) => {}
        }
        let (_, tx) = pending.remove(&id).expect("comprobado arriba");
        tx.send(answer)
            .map_err(|_| CmdError::invalid("el prompt ya no espera respuesta"))
    }

    /// Espera la respuesta; `None` = plazo vencido o cancelado (rechazo). Libera el id aunque
    /// la espera se cancele (la future se suelta).
    pub async fn wait(
        &self,
        id: PromptId,
        rx: oneshot::Receiver<PromptAnswer>,
        dur: Duration,
    ) -> Option<PromptAnswer> {
        let _release = Release { prompts: self, id };
        tokio::time::timeout(dur, rx)
            .await
            .ok()
            .and_then(Result::ok)
    }
}

struct Release<'a> {
    prompts: &'a Prompts,
    id: PromptId,
}

impl Drop for Release<'_> {
    fn drop(&mut self) {
        lock(&self.prompts.pending).remove(&self.id);
    }
}

/// Vigila un prompt mientras se espera: si termina sin respuesta, avisa a la UI para cerrar el modal.
struct PromptSlot<'a> {
    state: &'a AppState,
    id: PromptId,
    answered: bool,
}

impl Drop for PromptSlot<'_> {
    fn drop(&mut self) {
        lock(&self.state.prompts.pending).remove(&self.id);
        if !self.answered {
            self.state.emit(AppEvent::PromptDone { id: self.id });
        }
    }
}

/// `Arc` en cada campo: las tareas en segundo plano (terminales, prompts) poseen un clon.
#[derive(Default, Clone)]
pub struct AppState {
    pub terms: Arc<Registry<TermHandle>>,
    pub conns: Arc<Registry<ConnHandle>>,
    pub sftps: Arc<Registry<SftpHandle>>,
    /// Una cola por (sftp, raíz local).
    pub queues: Arc<Mutex<HashMap<(SftpId, String), QueueHandle>>>,
    /// Redirecciones activas por conexión: `(estado, handle de friki-ssh)`.
    pub fwds: Arc<Mutex<ForwardMap>>,
    pub prompts: Arc<Prompts>,
    /// Secuencia de colas (`QueueHandle::seq`): monótona aunque se liberen colas.
    pub queue_seq: Arc<AtomicU64>,
    events: Arc<Mutex<Option<Channel<AppEvent>>>>,
}

impl AppState {
    pub fn subscribe(&self, events: Channel<AppEvent>) {
        *lock(&self.events) = Some(events);
    }

    /// `false` si no hay suscriptor o el envío falló.
    pub fn emit(&self, ev: AppEvent) -> bool {
        lock(&self.events)
            .as_ref()
            .is_some_and(|c| c.send(ev).is_ok())
    }

    /// Conexión viva por id, o `notfound`.
    pub fn conn(&self, id: ConnId) -> Result<Arc<Connection>, CmdError> {
        self.conns
            .with(id, |c| c.conn.clone())
            .ok_or_else(|| CmdError::new(ErrorKind::Notfound, "conexión desconocida"))
    }

    /// Registra, avisa a la UI y espera la respuesta (`None` = rechazo).
    pub async fn ask(&self, prompt: Prompt, dur: Duration) -> Option<PromptAnswer> {
        let (id, rx) = self.prompts.register(prompt.kind());
        let mut slot = PromptSlot {
            state: self,
            id,
            answered: false,
        };
        if !self.emit(AppEvent::Prompt { id, prompt }) {
            return None;
        }
        let answer = self.prompts.wait(id, rx, dur).await;
        slot.answered = answer.is_some();
        answer
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dto::HostKeyDecision;
    use crate::error::ErrorKind;

    fn hostkey() -> PromptAnswer {
        PromptAnswer::HostKey {
            decision: HostKeyDecision::AcceptOnce,
        }
    }

    #[tokio::test]
    async fn respuesta_correcta_llega() {
        let p = Prompts::default();
        let (id, rx) = p.register(PromptKind::HostKey);
        assert!(id != 0 && id <= JS_SAFE_MASK);
        p.answer(id, hostkey()).unwrap();
        let a = p.wait(id, rx, Duration::from_secs(1)).await.unwrap();
        assert_eq!(a.kind(), PromptKind::HostKey);
    }

    #[tokio::test]
    async fn tipo_equivocado_es_invalid_y_no_consume() {
        let p = Prompts::default();
        let (id, rx) = p.register(PromptKind::Secret);
        let e = p.answer(id, hostkey()).unwrap_err();
        assert_eq!(e.kind, ErrorKind::Invalid);
        let ok = PromptAnswer::Secret {
            value: None,
            remember: false,
        };
        p.answer(id, ok).unwrap();
        assert!(p.wait(id, rx, Duration::from_secs(1)).await.is_some());
    }

    #[tokio::test]
    async fn doble_respuesta_e_id_desconocido() {
        let p = Prompts::default();
        let (id, _rx) = p.register(PromptKind::HostKey);
        p.answer(id, hostkey()).unwrap();
        assert_eq!(
            p.answer(id, hostkey()).unwrap_err().kind,
            ErrorKind::Invalid
        );
        assert_eq!(
            p.answer(999, hostkey()).unwrap_err().kind,
            ErrorKind::Invalid
        );
    }

    #[tokio::test]
    async fn plazo_vencido_es_rechazo_y_libera_el_id() {
        let p = Prompts::default();
        let (id, rx) = p.register(PromptKind::HostKey);
        assert!(p.wait(id, rx, Duration::from_millis(20)).await.is_none());
        assert_eq!(
            p.answer(id, hostkey()).unwrap_err().kind,
            ErrorKind::Invalid
        );
    }

    #[tokio::test]
    async fn ask_cancelado_libera_id_y_avisa_a_la_ui() {
        let state = AppState::default();
        let (tx, mut ui) = mpsc::unbounded_channel::<String>();
        state.subscribe(Channel::new(move |body| {
            if let tauri::ipc::InvokeResponseBody::Json(j) = body {
                let _ = tx.send(j);
            }
            Ok(())
        }));
        let p = Prompt::Passphrase {
            key_path: "/k".into(),
        };
        // La future se suelta por el plazo externo, como si term_close la cancelara.
        let fut = state.ask(p, Duration::from_secs(60));
        assert!(
            tokio::time::timeout(Duration::from_millis(30), fut)
                .await
                .is_err()
        );
        assert!(lock(&state.prompts.pending).is_empty());
        let shown: serde_json::Value = serde_json::from_str(&ui.recv().await.unwrap()).unwrap();
        let done: serde_json::Value = serde_json::from_str(&ui.recv().await.unwrap()).unwrap();
        assert_eq!(shown["type"], "prompt");
        assert_eq!(done["type"], "promptDone");
        assert_eq!(done["id"], shown["id"]);
    }

    #[test]
    fn registro_asigna_ids_distintos() {
        let r: Registry<u8> = Registry::default();
        let (a, b) = (r.insert(1), r.insert(2));
        assert_ne!(a, b);
        assert!(r.contains(a));
        assert_eq!(r.remove(a), Some(1));
        assert!(!r.contains(a));
    }
}
