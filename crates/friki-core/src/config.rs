//! Sesiones guardadas en TOML (una por fichero) e importación de `~/.ssh/config`.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::Result;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AuthMethod {
    Agent,
    PublicKey,
    KeyboardInteractive,
    Password,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LogConfig {
    /// Admite `%S` (sesión), `%H` (host) y `%D` (fecha AAAA-MM-DD).
    pub path: PathBuf,
    pub append: bool,
    /// Registrar también lo que teclea el usuario. Aun así, la entrada tras un
    /// aviso de contraseña nunca se escribe (TODO.md §3).
    pub log_input: bool,
}

impl Default for LogConfig {
    fn default() -> Self {
        Self { path: PathBuf::from("%S-%D.log"), append: true, log_input: false }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SessionConfig {
    pub name: String,
    pub host: String,
    pub port: u16,
    /// `None` = usuario local.
    pub user: Option<String>,
    /// Orden de intento.
    pub auth: Vec<AuthMethod>,
    pub identity_files: Vec<PathBuf>,
    pub proxy_jump: Option<String>,
    /// Solo tiene efecto si friki-ssh se compila con la feature `legacy`.
    pub legacy_algorithms: bool,
    pub term: String,
    /// Intentos máximos sumando todos los métodos (TODO.md §2).
    pub max_auth_attempts: u32,
    pub connect_timeout_secs: u64,
    pub keepalive_secs: Option<u64>,
    pub log: Option<LogConfig>,
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            name: String::new(),
            host: String::new(),
            port: 22,
            user: None,
            auth: vec![
                AuthMethod::Agent,
                AuthMethod::PublicKey,
                AuthMethod::KeyboardInteractive,
                AuthMethod::Password,
            ],
            identity_files: Vec::new(),
            proxy_jump: None,
            legacy_algorithms: false,
            term: "xterm-256color".into(),
            max_auth_attempts: 3,
            connect_timeout_secs: 15,
            keepalive_secs: Some(30),
            log: None,
        }
    }
}

/// Lee todos los `*.toml` de `dir` (inexistente = lista vacía), ordenados por nombre.
pub fn load_sessions(dir: &Path) -> Result<Vec<SessionConfig>> {
    let _ = dir;
    todo!("friki-core: load_sessions")
}

/// Escribe `<dir>/<name>.toml` de forma atómica y devuelve la ruta.
pub fn save_session(dir: &Path, session: &SessionConfig) -> Result<PathBuf> {
    let _ = (dir, session);
    todo!("friki-core: save_session")
}

/// Convierte los bloques `Host` concretos (sin comodines) de un `ssh_config`.
/// Soporta HostName, User, Port, IdentityFile (con `~`) y ProxyJump.
pub fn import_ssh_config(path: &Path) -> Result<Vec<SessionConfig>> {
    let _ = path;
    todo!("friki-core: import_ssh_config")
}

/// Resuelve un nombre de sesión guardada o `[user@]host[:port]`.
/// `None` si no es ni una sesión ni un destino válido.
pub fn resolve_target(sessions: &[SessionConfig], target: &str) -> Option<SessionConfig> {
    let _ = (sessions, target);
    todo!("friki-core: resolve_target")
}
