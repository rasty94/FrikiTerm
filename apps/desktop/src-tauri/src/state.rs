//! Estado de la app: registros de recursos vivos, ids y registro de prompts.

use std::collections::HashMap;
use std::hash::{BuildHasher, RandomState};
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use friki_ssh::Connection;
use tauri::ipc::Channel;
use tokio::sync::{mpsc, oneshot};

use crate::dto::{AppEvent, Prompt, PromptAnswer, PromptId, PromptKind, SftpId};
use crate::error::CmdError;

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

// Marcadores: T3 los sustituye por los recursos reales.
pub struct SftpHandle;
pub struct QueueHandle;

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

    /// Espera la respuesta; `None` = plazo vencido o cancelado (rechazo). Libera el id.
    pub async fn wait(
        &self,
        id: PromptId,
        rx: oneshot::Receiver<PromptAnswer>,
        dur: Duration,
    ) -> Option<PromptAnswer> {
        let r = tokio::time::timeout(dur, rx).await;
        lock(&self.pending).remove(&id);
        r.ok().and_then(Result::ok)
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
    pub prompts: Arc<Prompts>,
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

    /// Registra, avisa a la UI y espera la respuesta (`None` = rechazo).
    pub async fn ask(&self, prompt: Prompt, dur: Duration) -> Option<PromptAnswer> {
        let (id, rx) = self.prompts.register(prompt.kind());
        if !self.emit(AppEvent::Prompt { id, prompt }) {
            lock(&self.prompts.pending).remove(&id);
            return None;
        }
        self.prompts.wait(id, rx, dur).await
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
