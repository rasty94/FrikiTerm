//! Caché de listados para la sincronización (SQLite, tablas `config` y `direntries`, como
//! hace SecureFX según docs/13). Guarda el estado de la última sincronización para detectar
//! conflictos en modo `Both`.

use std::path::Path;

use crate::{Entry, Result};

pub struct DirCache {
    _private: (),
}

impl DirCache {
    /// Crea o abre la base de datos (permisos 0600 en Unix).
    pub fn open(path: &Path) -> Result<Self> {
        let _ = path;
        todo!("friki-xfer: DirCache::open")
    }

    /// Estado guardado de `root` (clave: etiqueta del fs + raíz).
    pub fn load(&self, key: &str) -> Result<Vec<Entry>> {
        let _ = key;
        todo!("friki-xfer: DirCache::load")
    }

    pub fn store(&self, key: &str, entries: &[Entry]) -> Result<()> {
        let _ = (key, entries);
        todo!("friki-xfer: DirCache::store")
    }
}
