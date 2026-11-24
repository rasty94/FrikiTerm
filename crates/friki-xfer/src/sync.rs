//! Sincronización de carpetas: comparación y plan de acciones con filtros (TODO.md §5).

use crate::{RemoteFs, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncMode {
    /// Local → remoto: sube lo nuevo o distinto.
    Upload,
    /// Remoto → local.
    Download,
    /// Ambos sentidos: gana la fecha más reciente.
    Both,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Filters {
    /// Patrones glob (`*.log`, `node_modules/`, `**/tmp/*`) relativos a la raíz.
    pub include: Vec<String>,
    pub exclude: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncOptions {
    pub mode: SyncMode,
    /// Borrar en el destino lo que no existe en el origen (no aplica a `Both`).
    pub delete: bool,
    pub filters: Filters,
    /// Comparar también por tamaño (siempre) y fecha con esta tolerancia en segundos.
    pub mtime_tolerance: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Upload {
        path: String,
    },
    Download {
        path: String,
    },
    MkdirRemote {
        path: String,
    },
    MkdirLocal {
        path: String,
    },
    DeleteRemote {
        path: String,
    },
    DeleteLocal {
        path: String,
    },
    /// Ambos cambiaron desde la última sincronización (solo con `Both` y caché).
    Conflict {
        path: String,
    },
}

/// Compara `local_root` y `remote_root` y devuelve el plan sin ejecutar nada.
/// Rutas del plan relativas a las raíces. Nombres remotos no seguros se ignoran.
pub async fn plan(
    local: &dyn RemoteFs,
    local_root: &str,
    remote: &dyn RemoteFs,
    remote_root: &str,
    opts: &SyncOptions,
    cache: Option<&crate::cache::DirCache>,
) -> Result<Vec<Action>> {
    let _ = (local, local_root, remote, remote_root, opts, cache);
    todo!("friki-xfer: sync::plan")
}
