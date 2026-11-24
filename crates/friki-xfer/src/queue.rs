//! Cola de transferencias: paralelismo, reintentos con reconexión, reanudación,
//! pausa, cancelación y límite de ancho de banda (TODO.md §5).

use std::sync::Arc;

use crate::RemoteFs;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Upload,
    Download,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resume {
    /// Continuar si el destino es más corto y coincide en tamaño parcial.
    Auto,
    Never,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Job {
    pub direction: Direction,
    pub src: String,
    pub dst: String,
    pub recursive: bool,
    pub resume: Resume,
    /// Conservar la fecha de modificación.
    pub preserve_mtime: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueueOptions {
    pub parallel: usize,
    pub retries: u32,
    /// Bytes por segundo en total; `None` = sin límite.
    pub bandwidth: Option<u64>,
}

impl Default for QueueOptions {
    fn default() -> Self {
        Self {
            parallel: 2,
            retries: 3,
            bandwidth: None,
        }
    }
}

pub type JobId = u64;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueueEvent {
    Started {
        id: JobId,
        file: String,
        total: Option<u64>,
    },
    Progress {
        id: JobId,
        file: String,
        done: u64,
    },
    Retrying {
        id: JobId,
        attempt: u32,
        error: String,
    },
    Finished {
        id: JobId,
        bytes: u64,
    },
    Failed {
        id: JobId,
        error: String,
    },
    Cancelled {
        id: JobId,
    },
}

pub struct Queue {
    _private: (),
}

impl Queue {
    /// `local` y `remote` son los dos extremos. `reconnect` se llama para obtener un
    /// `remote` nuevo tras un error de red antes de reintentar.
    pub fn new(
        local: Arc<dyn RemoteFs>,
        remote: Arc<dyn RemoteFs>,
        opts: QueueOptions,
        events: tokio::sync::mpsc::UnboundedSender<QueueEvent>,
    ) -> Self {
        let _ = (local, remote, opts, events);
        todo!("friki-xfer: Queue::new")
    }

    pub fn push(&self, job: Job) -> JobId {
        let _ = job;
        todo!("friki-xfer: Queue::push")
    }

    pub fn pause(&self, paused: bool) {
        let _ = paused;
        todo!("friki-xfer: Queue::pause")
    }

    pub fn cancel(&self, id: JobId) {
        let _ = id;
        todo!("friki-xfer: Queue::cancel")
    }

    /// Espera a que se vacíe la cola. Devuelve (correctos, fallidos).
    pub async fn wait(&self) -> (usize, usize) {
        todo!("friki-xfer: Queue::wait")
    }
}
