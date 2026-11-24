//! Definición de la línea de órdenes (la usa también `xtask` para man y completions).

use std::path::PathBuf;

use clap::{Parser, Subcommand};
use friki_core::config::SessionConfig;

#[derive(Debug, Parser)]
#[command(name = "friki", version, about = "Cliente SSH de terminal")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Abre una sesión interactiva en la TUI.
    Ssh(SshArgs),
    /// Ejecuta un comando remoto sin TUI y devuelve su estado.
    Exec(ExecArgs),
    /// Gestiona las sesiones guardadas.
    Sessions {
        #[command(subcommand)]
        action: SessionsAction,
    },
    /// Genera un par de claves Ed25519.
    Keygen(KeygenArgs),
}

/// Opciones de conexión comunes a `ssh` y `exec`.
#[derive(Debug, clap::Args)]
pub struct ConnArgs {
    /// Puerto (sobrescribe el de la sesión).
    #[arg(short, long)]
    pub port: Option<u16>,
    /// Usuario (sobrescribe el de la sesión).
    #[arg(short = 'l', long = "user")]
    pub user: Option<String>,
    /// Fichero de clave privada; repetible.
    #[arg(short, long = "identity")]
    pub identity: Vec<PathBuf>,
    /// Permite algoritmos heredados (requiere compilar con la feature `legacy`).
    #[arg(long)]
    pub legacy_algorithms: bool,
}

impl ConnArgs {
    pub fn apply(self, cfg: &mut SessionConfig) {
        if let Some(port) = self.port {
            cfg.port = port;
        }
        if let Some(user) = self.user {
            cfg.user = Some(user);
        }
        cfg.identity_files.extend(self.identity);
        if self.legacy_algorithms {
            cfg.legacy_algorithms = true;
        }
    }
}

#[derive(Debug, clap::Args)]
pub struct SshArgs {
    /// Sesión guardada o `[user@]host[:port]`.
    pub target: String,
    #[command(flatten)]
    pub conn: ConnArgs,
}

#[derive(Debug, clap::Args)]
pub struct ExecArgs {
    /// Sesión guardada o `[user@]host[:port]`.
    pub target: String,
    #[command(flatten)]
    pub conn: ConnArgs,
    /// Comando remoto; se une con espacios.
    #[arg(required = true, trailing_var_arg = true, allow_hyphen_values = true)]
    pub command: Vec<String>,
    /// Imprime `{"stdout","stderr","exit_status","exit_signal"}` en vez de la salida cruda.
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Subcommand)]
pub enum SessionsAction {
    /// Lista las sesiones guardadas.
    List {
        #[arg(long)]
        json: bool,
    },
    /// Guarda una sesión nueva.
    Add {
        name: String,
        /// `[user@]host[:port]`.
        target: String,
    },
    /// Importa los bloques `Host` concretos de un `ssh_config`.
    ImportSshConfig {
        /// Por defecto `~/.ssh/config`.
        path: Option<PathBuf>,
        /// Sobrescribe sesiones con el mismo nombre.
        #[arg(long)]
        force: bool,
    },
}

#[derive(Debug, clap::Args)]
pub struct KeygenArgs {
    /// Ruta de la clave privada (por defecto `~/.ssh/id_ed25519`).
    #[arg(short, long)]
    pub file: Option<PathBuf>,
    /// Comentario de la clave pública.
    #[arg(short = 'C', long, default_value = "frikiSSH")]
    pub comment: String,
    /// No pedir passphrase.
    #[arg(long)]
    pub no_passphrase: bool,
}
