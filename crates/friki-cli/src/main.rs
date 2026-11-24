//! Binario `friki`. Códigos de salida (TODO.md §6):
//! 0 correcto, 250 uso, 251 red, 252 autenticación, 253 clave de host, 254 otros.
//! Caben en 8 bits (Unix trunca módulo 256) y no chocan con los códigos habituales.

use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{anyhow, bail};
use clap::{Parser, Subcommand};
use friki_core::config::{self, SessionConfig};
use friki_core::known_hosts::{HostKey, HostKeyStatus, KnownHosts};
use friki_core::paths;
use friki_ssh::{AuthPrompter, Connection, HostKeyDecision, HostKeyVerifier};

#[derive(Debug, Parser)]
#[command(name = "friki", version, about = "Cliente SSH de terminal")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
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
struct ConnArgs {
    /// Puerto (sobrescribe el de la sesión).
    #[arg(short, long)]
    port: Option<u16>,
    /// Usuario (sobrescribe el de la sesión).
    #[arg(short = 'l', long = "user")]
    user: Option<String>,
    /// Fichero de clave privada; repetible.
    #[arg(short, long = "identity")]
    identity: Vec<PathBuf>,
    /// Permite algoritmos heredados (requiere compilar con la feature `legacy`).
    #[arg(long)]
    legacy_algorithms: bool,
}

impl ConnArgs {
    fn apply(self, cfg: &mut SessionConfig) {
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
struct SshArgs {
    /// Sesión guardada o `[user@]host[:port]`.
    target: String,
    #[command(flatten)]
    conn: ConnArgs,
}

#[derive(Debug, clap::Args)]
struct ExecArgs {
    /// Sesión guardada o `[user@]host[:port]`.
    target: String,
    #[command(flatten)]
    conn: ConnArgs,
    /// Comando remoto; se une con espacios.
    #[arg(required = true, trailing_var_arg = true, allow_hyphen_values = true)]
    command: Vec<String>,
    /// Imprime `{"stdout","stderr","exit_status"}` en vez de la salida cruda.
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Subcommand)]
enum SessionsAction {
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
struct KeygenArgs {
    /// Ruta de la clave privada (por defecto `~/.ssh/id_ed25519`).
    #[arg(short, long)]
    file: Option<PathBuf>,
    /// Comentario de la clave pública.
    #[arg(short = 'C', long, default_value = "frikiSSH")]
    comment: String,
    /// No pedir passphrase.
    #[arg(long)]
    no_passphrase: bool,
}

#[tokio::main]
async fn main() {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(e) => {
            let code = if e.use_stderr() { EXIT_USAGE } else { 0 };
            let _ = e.print();
            std::process::exit(code);
        }
    };
    match run(cli).await {
        Ok(code) => std::process::exit(code),
        Err(err) => {
            eprintln!("friki: {err:#}");
            std::process::exit(exit_code(&err));
        }
    }
}

async fn run(cli: Cli) -> anyhow::Result<i32> {
    match cli.command {
        Command::Ssh(args) => ssh(args).await,
        Command::Exec(args) => exec(args).await,
        Command::Sessions { action } => sessions(action).map(|()| 0),
        Command::Keygen(args) => keygen(args).map(|()| 0),
    }
}

const EXIT_USAGE: i32 = 250;
const EXIT_NETWORK: i32 = 251;
const EXIT_AUTH: i32 = 252;
const EXIT_HOST_KEY: i32 = 253;
const EXIT_OTHER: i32 = 254;

/// Código de salida de la CLI según el error (ARCHITECTURE.md).
pub fn exit_code(err: &anyhow::Error) -> i32 {
    if let Some(e) = err.downcast_ref::<friki_ssh::Error>() {
        return match e {
            friki_ssh::Error::Network(_) => EXIT_NETWORK,
            friki_ssh::Error::Auth(_) => EXIT_AUTH,
            friki_ssh::Error::HostKey(_) => EXIT_HOST_KEY,
            friki_ssh::Error::Negotiation(_) | friki_ssh::Error::Other(_) => EXIT_OTHER,
        };
    }
    if err.downcast_ref::<friki_core::Error>().is_some() {
        return EXIT_OTHER;
    }
    EXIT_OTHER
}

fn home_dir() -> anyhow::Result<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| anyhow!("no se encuentra el directorio HOME"))
}

fn load_target(target: &str) -> anyhow::Result<SessionConfig> {
    let sessions = config::load_sessions(&paths::sessions_dir())?;
    config::resolve_target(&sessions, target).ok_or_else(|| anyhow!("destino no válido: {target}"))
}

async fn ssh(args: SshArgs) -> anyhow::Result<i32> {
    let mut cfg = load_target(&args.target)?;
    args.conn.apply(&mut cfg);
    friki_tui::run(cfg).await
}

/// Acepta solo claves ya conocidas en `known_hosts`; desconocidas o cambiadas se rechazan.
struct StrictVerifier {
    known: KnownHosts,
}

#[async_trait::async_trait]
impl HostKeyVerifier for StrictVerifier {
    async fn verify(&self, host: &str, port: u16, key: &HostKey) -> HostKeyDecision {
        match self.known.check(host, port, key) {
            HostKeyStatus::Known => HostKeyDecision::AcceptOnce,
            status => {
                let motivo = match status {
                    HostKeyStatus::Changed { .. } => "clave cambiada",
                    HostKeyStatus::Revoked => "clave revocada",
                    _ => "clave desconocida",
                };
                eprintln!("friki: {motivo} para {host} ({})", key.fingerprint_sha256());
                HostKeyDecision::Reject
            }
        }
    }
}

/// Pide secretos por terminal sin eco; sin TTY no pregunta nada.
struct TerminalPrompter {
    interactive: bool,
}

/// Lee una línea (con eco) o una contraseña (sin eco) fuera del runtime async.
async fn ask(text: String, echo: bool) -> Option<String> {
    tokio::task::spawn_blocking(move || {
        if echo {
            eprint!("{text}");
            let mut line = String::new();
            std::io::stdin().read_line(&mut line).ok()?;
            Some(line.trim_end_matches(['\r', '\n']).to_string())
        } else {
            rpassword::prompt_password(text).ok()
        }
    })
    .await
    .ok()
    .flatten()
}

#[async_trait::async_trait]
impl AuthPrompter for TerminalPrompter {
    async fn password(&self, user: &str, host: &str) -> Option<String> {
        if !self.interactive {
            return None;
        }
        ask(format!("{user}@{host}'s password: "), false).await
    }

    async fn passphrase(&self, key_path: &Path) -> Option<String> {
        if !self.interactive {
            return None;
        }
        ask(format!("Passphrase for {}: ", key_path.display()), false).await
    }

    async fn keyboard_interactive(
        &self,
        name: &str,
        instructions: &str,
        prompts: &[(String, bool)],
    ) -> Option<Vec<String>> {
        if !self.interactive {
            return None;
        }
        if !name.is_empty() {
            eprintln!("{name}");
        }
        if !instructions.is_empty() {
            eprintln!("{instructions}");
        }
        let mut answers = Vec::with_capacity(prompts.len());
        for (text, echo) in prompts {
            answers.push(ask(text.clone(), *echo).await?);
        }
        Some(answers)
    }
}

async fn exec(args: ExecArgs) -> anyhow::Result<i32> {
    let mut cfg = load_target(&args.target)?;
    args.conn.apply(&mut cfg);
    let known = KnownHosts::load(&paths::known_hosts_file())?;
    let verifier = Arc::new(StrictVerifier { known });
    let prompter = Arc::new(TerminalPrompter {
        interactive: std::io::stdin().is_terminal(),
    });

    let conn = Connection::connect(&cfg, verifier, prompter).await?;
    let out = conn.exec(&args.command.join(" ")).await?;
    let _ = conn.disconnect().await;

    if args.json {
        let doc = serde_json::json!({
            "stdout": String::from_utf8_lossy(&out.stdout),
            "stderr": String::from_utf8_lossy(&out.stderr),
            "exit_status": out.exit_status,
        });
        println!("{doc}");
    } else {
        std::io::stdout().write_all(&out.stdout)?;
        std::io::stderr().write_all(&out.stderr)?;
    }

    match out.exit_status {
        // Los estados de salida de POSIX son de 8 bits.
        Some(status) => Ok((status & 0xff) as i32),
        None => bail!("el servidor no envió estado de salida"),
    }
}

fn describe(s: &SessionConfig) -> String {
    let user = s
        .user
        .as_deref()
        .map(|u| format!("{u}@"))
        .unwrap_or_default();
    format!("{}\t{user}{}:{}", s.name, s.host, s.port)
}

fn sessions(action: SessionsAction) -> anyhow::Result<()> {
    let dir = paths::sessions_dir();
    match action {
        SessionsAction::List { json } => {
            let sessions = config::load_sessions(&dir)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&sessions)?);
            } else {
                for s in &sessions {
                    println!("{}", describe(s));
                }
            }
        }
        SessionsAction::Add { name, target } => {
            if config::load_sessions(&dir)?.iter().any(|s| s.name == name) {
                bail!("la sesión '{name}' ya existe");
            }
            let mut session = config::resolve_target(&[], &target)
                .ok_or_else(|| anyhow!("destino no válido: {target}"))?;
            session.name = name.clone();
            let path = config::save_session(&dir, &session)?;
            println!("Sesión '{name}' guardada en {}", path.display());
        }
        SessionsAction::ImportSshConfig { path, force } => {
            let path = match path {
                Some(p) => p,
                None => home_dir()?.join(".ssh").join("config"),
            };
            let imported = config::import_ssh_config(&path)?;
            let existing: Vec<String> = config::load_sessions(&dir)?
                .into_iter()
                .map(|s| s.name)
                .collect();
            let (mut escritas, mut omitidas) = (0usize, 0usize);
            for s in &imported {
                if !force && existing.contains(&s.name) {
                    omitidas += 1;
                } else {
                    config::save_session(&dir, s)?;
                    escritas += 1;
                }
            }
            println!(
                "Importadas {escritas} sesiones de {} ({omitidas} omitidas por existir; usa --force para sobrescribir)",
                path.display()
            );
        }
    }
    Ok(())
}

/// Pide la passphrase dos veces sin eco; vacía = sin passphrase.
fn read_new_passphrase() -> anyhow::Result<Option<String>> {
    if !std::io::stdin().is_terminal() {
        bail!("se necesita una terminal para pedir la passphrase (o usa --no-passphrase)");
    }
    let first = rpassword::prompt_password("Passphrase (vacía = ninguna): ")?;
    let second = rpassword::prompt_password("Repite la passphrase: ")?;
    if first != second {
        bail!("las passphrases no coinciden");
    }
    Ok(if first.is_empty() { None } else { Some(first) })
}

fn keygen(args: KeygenArgs) -> anyhow::Result<()> {
    let path = match args.file {
        Some(p) => p,
        None => home_dir()?.join(".ssh").join("id_ed25519"),
    };
    let public = PathBuf::from(format!("{}.pub", path.display()));
    if path.exists() || public.exists() {
        bail!("{} ya existe; no se sobrescribe", path.display());
    }
    let passphrase = if args.no_passphrase {
        None
    } else {
        read_new_passphrase()?
    };
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let line = friki_ssh::keys::generate_ed25519(&path, &args.comment, passphrase.as_deref())?;
    println!("{line}");
    eprintln!("Clave privada en {}", path.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Command {
        Cli::try_parse_from(args)
            .expect("argumentos válidos")
            .command
    }

    #[test]
    fn exit_code_red_es_100() {
        let e = anyhow::Error::from(friki_ssh::Error::Network("x".into()));
        assert_eq!(exit_code(&e), EXIT_NETWORK);
    }

    #[test]
    fn exit_code_auth_es_200() {
        let e = anyhow::Error::from(friki_ssh::Error::Auth("x".into()));
        assert_eq!(exit_code(&e), EXIT_AUTH);
    }

    #[test]
    fn exit_code_clave_de_host_es_300() {
        let e = anyhow::Error::from(friki_ssh::Error::HostKey("x".into()));
        assert_eq!(exit_code(&e), EXIT_HOST_KEY);
    }

    #[test]
    fn exit_code_negociacion_y_otros_son_400() {
        let neg = anyhow::Error::from(friki_ssh::Error::Negotiation("x".into()));
        let otro = anyhow::Error::from(friki_ssh::Error::Other("x".into()));
        assert_eq!(exit_code(&neg), EXIT_OTHER);
        assert_eq!(exit_code(&otro), EXIT_OTHER);
    }

    #[test]
    fn exit_code_error_de_core_es_400() {
        let e = anyhow::Error::from(friki_core::Error::Secret("x".into()));
        assert_eq!(exit_code(&e), EXIT_OTHER);
    }

    #[test]
    fn exit_code_error_ajeno_es_400() {
        assert_eq!(exit_code(&anyhow!("otro")), EXIT_OTHER);
    }

    #[test]
    fn exit_code_atraviesa_contexto() {
        let e = anyhow::Error::from(friki_ssh::Error::Auth("x".into())).context("conectando");
        assert_eq!(exit_code(&e), EXIT_AUTH);
    }

    #[test]
    fn parsea_ssh_con_opciones() {
        match parse(&[
            "friki",
            "ssh",
            "-p",
            "2222",
            "-l",
            "ana",
            "-i",
            "a",
            "--identity",
            "b",
            "--legacy-algorithms",
            "srv",
        ]) {
            Command::Ssh(a) => {
                assert_eq!(a.target, "srv");
                assert_eq!(a.conn.port, Some(2222));
                assert_eq!(a.conn.user.as_deref(), Some("ana"));
                assert_eq!(
                    a.conn.identity,
                    vec![PathBuf::from("a"), PathBuf::from("b")]
                );
                assert!(a.conn.legacy_algorithms);
            }
            other => panic!("subcomando inesperado: {other:?}"),
        }
    }

    #[test]
    fn parsea_ssh_minimo() {
        match parse(&["friki", "ssh", "ana@host:22"]) {
            Command::Ssh(a) => {
                assert_eq!(a.target, "ana@host:22");
                assert!(
                    a.conn.port.is_none() && a.conn.user.is_none() && a.conn.identity.is_empty()
                );
                assert!(!a.conn.legacy_algorithms);
            }
            other => panic!("subcomando inesperado: {other:?}"),
        }
    }

    #[test]
    fn parsea_exec_con_comando_y_guiones() {
        match parse(&["friki", "exec", "--json", "srv", "ls", "-la", "/tmp"]) {
            Command::Exec(a) => {
                assert_eq!(a.target, "srv");
                assert_eq!(a.command, vec!["ls", "-la", "/tmp"]);
                assert!(a.json);
            }
            other => panic!("subcomando inesperado: {other:?}"),
        }
    }

    #[test]
    fn exec_exige_comando() {
        assert!(Cli::try_parse_from(["friki", "exec", "srv"]).is_err());
    }

    #[test]
    fn parsea_sessions_list() {
        match parse(&["friki", "sessions", "list", "--json"]) {
            Command::Sessions {
                action: SessionsAction::List { json },
            } => assert!(json),
            other => panic!("subcomando inesperado: {other:?}"),
        }
    }

    #[test]
    fn parsea_sessions_add() {
        match parse(&["friki", "sessions", "add", "web", "ana@h:2200"]) {
            Command::Sessions {
                action: SessionsAction::Add { name, target },
            } => {
                assert_eq!(name, "web");
                assert_eq!(target, "ana@h:2200");
            }
            other => panic!("subcomando inesperado: {other:?}"),
        }
    }

    #[test]
    fn parsea_import_ssh_config() {
        match parse(&["friki", "sessions", "import-ssh-config"]) {
            Command::Sessions {
                action: SessionsAction::ImportSshConfig { path, force },
            } => {
                assert!(path.is_none() && !force);
            }
            other => panic!("subcomando inesperado: {other:?}"),
        }
        match parse(&[
            "friki",
            "sessions",
            "import-ssh-config",
            "/x/config",
            "--force",
        ]) {
            Command::Sessions {
                action: SessionsAction::ImportSshConfig { path, force },
            } => {
                assert_eq!(path, Some(PathBuf::from("/x/config")));
                assert!(force);
            }
            other => panic!("subcomando inesperado: {other:?}"),
        }
    }

    #[test]
    fn parsea_keygen() {
        match parse(&[
            "friki",
            "keygen",
            "-f",
            "/k/id",
            "-C",
            "yo",
            "--no-passphrase",
        ]) {
            Command::Keygen(a) => {
                assert_eq!(a.file, Some(PathBuf::from("/k/id")));
                assert_eq!(a.comment, "yo");
                assert!(a.no_passphrase);
            }
            other => panic!("subcomando inesperado: {other:?}"),
        }
        match parse(&["friki", "keygen"]) {
            Command::Keygen(a) => {
                assert!(a.file.is_none() && a.comment == "frikiSSH" && !a.no_passphrase);
            }
            other => panic!("subcomando inesperado: {other:?}"),
        }
    }
}
