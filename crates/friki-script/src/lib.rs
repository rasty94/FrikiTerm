//! Scripting con Rhai (TODO.md §7). La API imita la de SecureCRT (docs/12) en lo esencial:
//! `crt.Screen.Send`, `WaitForString(s)`, `ReadString`, `Get`, `crt.Session.Connect`, …
//! con timeouts obligatorios y sandbox: sin disco ni red salvo permiso explícito.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

/// Lo que el script puede hacer sobre la sesión. La TUI lo implementa para un panel.
#[async_trait::async_trait]
pub trait ScriptHost: Send + Sync {
    async fn send(&self, text: &str) -> Result<(), String>;
    /// Índice de la cadena encontrada, o `None` si vence el timeout.
    async fn wait_for_strings(
        &self,
        needles: &[String],
        timeout: Duration,
    ) -> Result<Option<usize>, String>;
    /// Texto recibido hasta (sin incluir) una de `until`; `None` si vence el timeout.
    async fn read_string(
        &self,
        until: &[String],
        timeout: Duration,
    ) -> Result<Option<String>, String>;
    /// Filas visibles como texto.
    fn screen_text(&self) -> Vec<String>;
    /// (fila, columna), base 1 como en SecureCRT.
    fn cursor(&self) -> (u16, u16);
    async fn connect(&self, target: &str) -> Result<(), String>;
    async fn disconnect(&self) -> Result<(), String>;
    fn connected(&self) -> bool;
    /// Mensaje para el usuario (barra de estado o diálogo).
    fn message(&self, text: &str);
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Permissions {
    /// Directorios que el script puede leer.
    pub read: Vec<PathBuf>,
    /// Directorios donde puede escribir.
    pub write: Vec<PathBuf>,
    /// Puede abrir o cerrar conexiones.
    pub connect: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Limits {
    pub max_operations: u64,
    pub max_runtime: Duration,
    /// Timeout por defecto de las esperas si el script no da uno.
    pub default_wait: Duration,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_operations: 10_000_000,
            max_runtime: Duration::from_secs(3600),
            default_wait: Duration::from_secs(30),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ScriptError {
    #[error("compilación: {0}")]
    Compile(String),
    #[error("ejecución: {0}")]
    Runtime(String),
    #[error("permiso denegado: {0}")]
    Denied(String),
    #[error("límite superado: {0}")]
    Limit(String),
}

/// Ejecuta el script con `args` (`crt.Arguments`). Devuelve el valor de `crt.Quit(n)` o 0.
pub async fn run(
    source: &str,
    args: &[String],
    host: Arc<dyn ScriptHost>,
    perms: &Permissions,
    limits: &Limits,
) -> Result<i64, ScriptError> {
    let _ = (source, args, host, perms, limits);
    todo!("friki-script: run")
}
