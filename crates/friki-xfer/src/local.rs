//! `RemoteFs` sobre el sistema de ficheros local (pruebas y lado local de la cola).

use std::path::PathBuf;

/// Raíz desde la que se resuelven las rutas; nada fuera de ella es accesible.
pub struct LocalFs {
    pub root: PathBuf,
}
