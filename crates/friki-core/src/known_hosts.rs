//! `known_hosts` compatible con OpenSSH: lee entradas en claro y hasheadas
//! (`|1|salt|hash`), `[host]:port` para puertos distintos de 22, y marcadores
//! `@revoked`. Escribe en claro, con bloqueo y de forma atómica.

use std::path::{Path, PathBuf};

use crate::Result;

/// Clave pública de host en formato OpenSSH (`algoritmo base64`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostKey {
    pub algorithm: String,
    pub base64: String,
}

impl HostKey {
    /// `SHA256:<base64 sin relleno>`, igual que `ssh-keygen -lf`.
    pub fn fingerprint_sha256(&self) -> String {
        todo!("friki-core: fingerprint_sha256")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostKeyStatus {
    Known,
    Unknown,
    /// Hay otra clave del mismo algoritmo para ese host.
    Changed { previous: HostKey },
    Revoked,
}

pub struct KnownHosts {
    path: PathBuf,
}

impl KnownHosts {
    /// Fichero inexistente = vacío.
    pub fn load(path: &Path) -> Result<Self> {
        let _ = path;
        todo!("friki-core: KnownHosts::load")
    }

    pub fn check(&self, host: &str, port: u16, key: &HostKey) -> HostKeyStatus {
        let _ = (host, port, key, &self.path);
        todo!("friki-core: KnownHosts::check")
    }

    pub fn add(&mut self, host: &str, port: u16, key: &HostKey) -> Result<()> {
        let _ = (host, port, key);
        todo!("friki-core: KnownHosts::add")
    }
}
