//! Definición de la línea de órdenes (la usa también `xtask` para man y completions).

use std::path::PathBuf;

use clap::{Parser, Subcommand};
use friki_core::config::{self, Forward, SessionConfig};

#[derive(Debug, Parser)]
#[command(name = "friki", version, about = "Cliente SSH de terminal")]
pub struct Cli {
    /// Sin subcomando abre la TUI con el selector de sesiones.
    #[command(subcommand)]
    pub command: Option<Command>,
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
    /// Mantiene redirecciones de puertos (-L/-R/-D) sin TUI hasta Ctrl+C.
    Fwd(FwdArgs),
    /// Lista un directorio remoto por SFTP: `destino:ruta`.
    Ls(LsArgs),
    /// Descarga `destino:ruta` por SFTP.
    Get(GetArgs),
    /// Sube ficheros a `destino:ruta` por SFTP.
    Put(PutArgs),
    /// Copia estilo scp entre local y `destino:ruta` (SFTP; `--scp` fuerza SCP).
    Cp(CpArgs),
    /// Sincroniza una carpeta local con una remota por SFTP.
    Sync(SyncArgs),
    /// FTP y FTPS.
    Ftp {
        #[command(subcommand)]
        action: FtpAction,
    },
    /// Instala una clave pública en `~/.ssh/authorized_keys` (como ssh-copy-id).
    PubkeyInstall(PubkeyArgs),
    /// Ejecuta un script Rhai sin TUI sobre una sesión SSH.
    Script(ScriptArgs),
    /// Abre un puerto serie en la TUI, o con `list` lista los disponibles.
    Serial(SerialArgs),
    /// Abre una sesión Telnet en la TUI.
    Telnet(TelnetArgs),
    /// Abre un shell local (o la orden dada) en la TUI.
    Shell(ShellArgs),
}

/// Separa `a:b:c` en los `:` fuera de corchetes; los corchetes (IPv6) se quitan.
fn split_colon(s: &str) -> Option<Vec<String>> {
    let (mut out, mut cur, mut in_br) = (Vec::new(), String::new(), false);
    for c in s.chars() {
        match c {
            '[' if !in_br && cur.is_empty() => in_br = true,
            ']' if in_br => in_br = false,
            ':' if !in_br => out.push(std::mem::take(&mut cur)),
            c => cur.push(c),
        }
    }
    (!in_br).then(|| {
        out.push(cur);
        out
    })
}

fn port(s: &str) -> Result<u16, String> {
    s.parse().map_err(|_| format!("puerto no válido: `{s}`"))
}

/// `[bind:]puerto:host:puerto_host` (OpenSSH). Devuelve (bind, puerto, host, puerto_host).
fn parse_spec(s: &str) -> Result<(String, u16, String, u16), String> {
    let err = || format!("`{s}`: se esperaba [bind:]puerto:host:puerto_host");
    let t = split_colon(s).ok_or_else(err)?;
    let (bind, rest) = match t.as_slice() {
        [_, _, _] => (String::new(), &t[..]),
        [b, rest @ ..] if rest.len() == 3 => (b.clone(), rest),
        _ => return Err(err()),
    };
    if rest[1].is_empty() {
        return Err(err());
    }
    Ok((bind, port(&rest[0])?, rest[1].clone(), port(&rest[2])?))
}

pub fn parse_local(s: &str) -> Result<Forward, String> {
    let (bind, port, host, host_port) = parse_spec(s)?;
    Ok(Forward::Local {
        bind,
        port,
        host,
        host_port,
    })
}

pub fn parse_remote(s: &str) -> Result<Forward, String> {
    let (bind, port, host, host_port) = parse_spec(s)?;
    Ok(Forward::Remote {
        bind,
        port,
        host,
        host_port,
    })
}

/// `[bind:]puerto` (SOCKS5).
pub fn parse_dynamic(s: &str) -> Result<Forward, String> {
    let err = || format!("`{s}`: se esperaba [bind:]puerto");
    match split_colon(s).ok_or_else(err)?.as_slice() {
        [p] => Ok(Forward::Dynamic {
            bind: String::new(),
            port: port(p)?,
        }),
        [b, p] => Ok(Forward::Dynamic {
            bind: b.clone(),
            port: port(p)?,
        }),
        _ => Err(err()),
    }
}

/// `[user@]host[:port][,…]`; cada salto debe ser un destino válido.
pub fn parse_jump(s: &str) -> Result<String, String> {
    if s.split(',')
        .any(|hop| config::resolve_target(&[], hop).is_none())
    {
        return Err(format!("`{s}`: se esperaba [user@]host[:port][,…]"));
    }
    Ok(s.to_string())
}

/// Opciones de conexión comunes a `ssh`, `exec`, `fwd`, transferencias y `script`.
#[derive(Debug, Clone, clap::Args)]
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
    /// Saltos ProxyJump: `[user@]host[:port][,…]`.
    #[arg(short = 'J', long = "jump", value_parser = parse_jump)]
    pub jump: Option<String>,
    /// Redirección local `[bind:]puerto:host:puerto_host`; repetible.
    #[arg(short = 'L', value_name = "REDIR", value_parser = parse_local)]
    pub fwd_local: Vec<Forward>,
    /// Redirección remota `[bind:]puerto:host:puerto_host`; repetible.
    #[arg(short = 'R', value_name = "REDIR", value_parser = parse_remote)]
    pub fwd_remote: Vec<Forward>,
    /// Proxy SOCKS5 dinámico `[bind:]puerto`; repetible.
    #[arg(short = 'D', value_name = "REDIR", value_parser = parse_dynamic)]
    pub fwd_dynamic: Vec<Forward>,
    /// Reenvía el agente SSH al remoto.
    #[arg(short = 'A', long = "forward-agent")]
    pub forward_agent: bool,
    /// Aborta si el servidor no anuncia strict KEX (Terrapin).
    #[arg(long)]
    pub require_strict_kex: bool,
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
        if self.jump.is_some() {
            cfg.proxy_jump = self.jump;
        }
        cfg.forwards.extend(self.fwd_local);
        cfg.forwards.extend(self.fwd_remote);
        cfg.forwards.extend(self.fwd_dynamic);
        cfg.forward_agent |= self.forward_agent;
        cfg.require_strict_kex |= self.require_strict_kex;
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
    #[arg(short = 'C', long, default_value = "FrikiTerm")]
    pub comment: String,
    /// No pedir passphrase.
    #[arg(long)]
    pub no_passphrase: bool,
}

#[derive(Debug, clap::Args)]
pub struct FwdArgs {
    /// Sesión guardada o `[user@]host[:port]`.
    pub target: String,
    #[command(flatten)]
    pub conn: ConnArgs,
}

#[derive(Debug, clap::Args)]
pub struct LsArgs {
    /// `destino:ruta` (ruta vacía = directorio personal).
    pub remote: String,
    #[command(flatten)]
    pub conn: ConnArgs,
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, clap::Args)]
pub struct GetArgs {
    /// `destino:ruta`.
    pub remote: String,
    /// Fichero o directorio local (por defecto, el directorio actual).
    pub local: Option<PathBuf>,
    /// Descarga directorios completos.
    #[arg(short = 'r', long)]
    pub recursive: bool,
    #[command(flatten)]
    pub conn: ConnArgs,
    /// Eventos de progreso como JSON (uno por línea) en stdout.
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, clap::Args)]
pub struct PutArgs {
    /// Ficheros locales seguidos de `destino:ruta`.
    #[arg(required = true, num_args = 2..)]
    pub paths: Vec<String>,
    #[arg(short = 'r', long)]
    pub recursive: bool,
    #[command(flatten)]
    pub conn: ConnArgs,
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, clap::Args)]
pub struct CpArgs {
    /// Orígenes y destino; exactamente un lado es `destino:ruta`.
    #[arg(required = true, num_args = 2..)]
    pub paths: Vec<String>,
    #[arg(short = 'r', long)]
    pub recursive: bool,
    /// Usa el protocolo SCP en vez de SFTP.
    #[arg(long)]
    pub scp: bool,
    #[command(flatten)]
    pub conn: ConnArgs,
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, clap::Args)]
pub struct SyncArgs {
    /// Directorio local.
    pub local: PathBuf,
    /// `destino:ruta` del directorio remoto.
    pub remote: String,
    /// Remoto a local.
    #[arg(long, conflicts_with = "both")]
    pub down: bool,
    /// En ambos sentidos (gana la fecha más reciente).
    #[arg(long)]
    pub both: bool,
    /// Borra en el destino lo que no está en el origen.
    #[arg(long)]
    pub delete: bool,
    /// Solo ficheros que cumplan el glob; repetible.
    #[arg(long)]
    pub include: Vec<String>,
    /// Ignora lo que cumpla el glob; repetible.
    #[arg(long)]
    pub exclude: Vec<String>,
    /// Imprime el plan y no cambia nada.
    #[arg(long)]
    pub dry_run: bool,
    /// No pedir confirmación con más de 20 borrados.
    #[arg(long)]
    pub yes: bool,
    #[command(flatten)]
    pub conn: ConnArgs,
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Subcommand)]
pub enum FtpAction {
    /// Lista un directorio.
    Ls {
        #[command(flatten)]
        ftp: FtpConn,
        /// Ruta remota.
        path: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Descarga un fichero (o `-r` un directorio).
    Get {
        #[command(flatten)]
        ftp: FtpConn,
        remote: String,
        local: Option<PathBuf>,
        #[arg(short = 'r', long)]
        recursive: bool,
        #[arg(long)]
        json: bool,
    },
    /// Sube un fichero (o `-r` un directorio).
    Put {
        #[command(flatten)]
        ftp: FtpConn,
        local: PathBuf,
        /// Ruta remota (por defecto, el nombre del fichero).
        remote: Option<String>,
        #[arg(short = 'r', long)]
        recursive: bool,
        #[arg(long)]
        json: bool,
    },
}

/// Conexión FTP. La contraseña va en `FRIKI_FTP_PASSWORD` o se pide por terminal.
#[derive(Debug, clap::Args)]
pub struct FtpConn {
    /// `host[:puerto]`.
    pub host: String,
    #[arg(short = 'u', long, default_value = "anonymous")]
    pub user: String,
    /// FTPS explícito (AUTH TLS).
    #[arg(long)]
    pub tls: bool,
    /// Permite usuario y contraseña en claro (sin `--tls`) para un usuario no anónimo.
    #[arg(long)]
    pub insecure_plaintext: bool,
}

#[derive(Debug, clap::Args)]
pub struct PubkeyArgs {
    /// Sesión guardada o `[user@]host[:port]`.
    pub target: String,
    /// Clave pública a instalar (por defecto la primera de ~/.ssh/id_*.pub).
    #[arg(short = 'i', long = "identity")]
    pub pubkey: Option<PathBuf>,
    /// Clave privada para autenticarse; repetible.
    #[arg(short = 'I', long = "auth-identity")]
    pub auth_identity: Vec<PathBuf>,
    #[arg(short, long)]
    pub port: Option<u16>,
    #[arg(short = 'l', long = "user")]
    pub user: Option<String>,
    #[arg(short = 'J', long = "jump", value_parser = parse_jump)]
    pub jump: Option<String>,
    #[arg(long)]
    pub legacy_algorithms: bool,
    #[arg(long)]
    pub require_strict_kex: bool,
}

#[derive(Debug, clap::Args)]
pub struct ScriptArgs {
    /// Fichero `.rhai`.
    pub file: PathBuf,
    /// Sesión SSH sobre la que corre (sin ella, el script no tiene pantalla).
    #[arg(long)]
    pub target: Option<String>,
    /// Directorio que el script puede leer; repetible.
    #[arg(long)]
    pub allow_read: Vec<PathBuf>,
    /// Directorio donde el script puede escribir; repetible.
    #[arg(long)]
    pub allow_write: Vec<PathBuf>,
    /// Permite `crt.Session.Connect/Disconnect`.
    #[arg(long)]
    pub allow_connect: bool,
    #[command(flatten)]
    pub conn: ConnArgs,
    /// Argumentos del script (`crt.Arguments`), tras `--`.
    #[arg(last = true)]
    pub args: Vec<String>,
}

fn parse_parity(s: &str) -> Result<config::Parity, String> {
    match s {
        "none" => Ok(config::Parity::None),
        "odd" => Ok(config::Parity::Odd),
        "even" => Ok(config::Parity::Even),
        _ => Err("paridad: none, odd o even".into()),
    }
}

fn parse_flow(s: &str) -> Result<config::FlowControl, String> {
    match s {
        "none" => Ok(config::FlowControl::None),
        "software" => Ok(config::FlowControl::Software),
        "hardware" => Ok(config::FlowControl::Hardware),
        _ => Err("control de flujo: none, software o hardware".into()),
    }
}

#[derive(Debug, clap::Args)]
pub struct SerialArgs {
    /// Dispositivo (`/dev/ttyUSB0`, `COM3`) o `list` para listar los puertos.
    // ponytail: un dispositivo llamado literalmente `list` se escribe `./list`.
    pub device: String,
    #[arg(short, long, default_value_t = 115_200)]
    pub baud: u32,
    #[arg(long, default_value_t = 8, value_parser = clap::value_parser!(u8).range(5..=8))]
    pub data_bits: u8,
    #[arg(long, default_value = "none", value_parser = parse_parity)]
    pub parity: config::Parity,
    #[arg(long, default_value_t = 1, value_parser = clap::value_parser!(u8).range(1..=2))]
    pub stop_bits: u8,
    #[arg(long, default_value = "none", value_parser = parse_flow)]
    pub flow: config::FlowControl,
}

#[derive(Debug, clap::Args)]
pub struct TelnetArgs {
    /// `host[:puerto]` (23, o 992 con `--tls`).
    pub host: String,
    /// Telnet sobre TLS con verificación de certificado.
    #[arg(long)]
    pub tls: bool,
}

#[derive(Debug, clap::Args)]
pub struct ShellArgs {
    /// Orden a ejecutar (por defecto `$SHELL`).
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    pub command: Vec<String>,
}
