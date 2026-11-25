//! Binario `friki`. Códigos de salida (TODO.md §6):
//! 0 correcto, 250 uso, 251 red, 252 autenticación, 253 clave de host, 254 otros.
//! Caben en 8 bits (Unix trunca módulo 256) y no chocan con los códigos habituales.

use std::future::Future;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;

use anyhow::{anyhow, bail};
use clap::Parser;
use friki_cli::cli::{Cli, Command, ExecArgs, KeygenArgs, SessionsAction, SshArgs};
use friki_core::config::{self, SessionConfig};

mod net;
mod pubkey;
mod script;
mod xfer;
use friki_core::known_hosts::{HostKey, HostKeyStatus};
use friki_core::paths;
use friki_ssh::{AuthPrompter, Connection, HostKeyDecision, KnownHostsVerifier, Zeroizing};
use tokio::io::AsyncWriteExt;

fn main() {
    // Antes de crear hilos: `time` solo da el desfase local en un proceso monohilo.
    friki_term::init_local_offset();
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("runtime tokio")
        .block_on(amain());
}

async fn amain() {
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
            eprintln!("friki: {}", sanitize_remote(&format!("{err:#}")));
            std::process::exit(exit_code(&err));
        }
    }
}

/// Error de uso (destino o argumentos no válidos): sale con `EXIT_USAGE`.
#[derive(Debug)]
struct UsageError(String);

impl std::fmt::Display for UsageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for UsageError {}

/// Quita del texto remoto (nombres, instrucciones, prompts, avisos) los caracteres
/// de control C0, DEL y C1, para que un servidor no inyecte secuencias de escape en
/// la terminal. Cada control se sustituye por un espacio, así `\n` no rompe la línea.
fn sanitize_remote(texto: &str) -> String {
    texto
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}

async fn run(cli: Cli) -> anyhow::Result<i32> {
    let Some(command) = cli.command else {
        return friki_tui::run_app(None, friki_tui::Paths::default_paths()).await;
    };
    match command {
        Command::Ssh(args) => ssh(args).await,
        Command::Exec(args) => exec(args).await,
        Command::Sessions { action } => sessions(action).map(|()| 0),
        Command::Keygen(args) => keygen(args).map(|()| 0),
        Command::Fwd(args) => net::fwd(args).await,
        Command::Ls(args) => xfer::ls(args).await,
        Command::Get(args) => xfer::get(args).await,
        Command::Put(args) => xfer::put(args).await,
        Command::Cp(args) => xfer::cp(args).await,
        Command::Sync(args) => xfer::sync(args).await,
        Command::Ftp { action } => xfer::ftp(action).await,
        Command::PubkeyInstall(args) => pubkey::install(args).await,
        Command::Script(args) => script::run(args).await,
        Command::Serial(args) => net::serial(args).await,
        Command::Telnet(args) => net::telnet(args).await,
        Command::Shell(args) => net::shell(args).await,
        Command::TftpServer(args) => net::tftp_server(args).await,
    }
}

const EXIT_USAGE: i32 = 250;
const EXIT_NETWORK: i32 = 251;
const EXIT_AUTH: i32 = 252;
const EXIT_HOST_KEY: i32 = 253;
const EXIT_OTHER: i32 = 254;

/// Error con código de salida ya decidido (fallos de la cola de transferencias).
#[derive(Debug)]
struct CodedError(i32, String);

impl std::fmt::Display for CodedError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.1)
    }
}

impl std::error::Error for CodedError {}

fn ssh_code(e: &friki_ssh::Error) -> i32 {
    match e {
        friki_ssh::Error::Network(_) => EXIT_NETWORK,
        friki_ssh::Error::Auth(_) => EXIT_AUTH,
        friki_ssh::Error::HostKey(_) => EXIT_HOST_KEY,
        friki_ssh::Error::Negotiation(_) | friki_ssh::Error::Other(_) => EXIT_OTHER,
    }
}

/// Código de un error de `friki-xfer`: red/E/S → 251; permisos, no existe y el resto → 254.
fn xfer_code(e: &friki_xfer::Error) -> i32 {
    use std::io::ErrorKind::{NotFound, PermissionDenied};
    match e {
        friki_xfer::Error::Ssh(e) => ssh_code(e),
        friki_xfer::Error::Io(e) if matches!(e.kind(), NotFound | PermissionDenied) => EXIT_OTHER,
        friki_xfer::Error::Io(_) => EXIT_NETWORK,
        // Refusal de seguridad de `sync --both` (como el de `--yes`): uso incorrecto.
        friki_xfer::Error::UnsafeSync(_) => EXIT_USAGE,
        _ => EXIT_OTHER,
    }
}

/// Único punto que traduce un fallo de la cola a código de salida (251/254, o 252/253).
/// Hoy se deduce del texto; con `QueueEvent::Failed { kind }` se mapea `kind` aquí
/// (Network → 251, el resto → 254) y el texto desaparece.
fn failure_code(kind: friki_xfer::ErrorKind) -> i32 {
    match kind {
        friki_xfer::ErrorKind::Network => EXIT_NETWORK,
        _ => EXIT_OTHER,
    }
}

/// Código de salida de la CLI según el error (ARCHITECTURE.md).
pub fn exit_code(err: &anyhow::Error) -> i32 {
    if err.downcast_ref::<UsageError>().is_some() {
        return EXIT_USAGE;
    }
    if let Some(e) = err.downcast_ref::<CodedError>() {
        return e.0;
    }
    if let Some(e) = err.downcast_ref::<friki_xfer::Error>() {
        return xfer_code(e);
    }
    if let Some(e) = err.downcast_ref::<friki_ssh::Error>() {
        return ssh_code(e);
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
    resolver_destino(&sessions, target)
}

fn resolver_destino(sessions: &[SessionConfig], target: &str) -> anyhow::Result<SessionConfig> {
    config::resolve_target(sessions, target)
        .ok_or_else(|| UsageError(format!("destino no válido: {target}")).into())
}

async fn ssh(args: SshArgs) -> anyhow::Result<i32> {
    let mut cfg = load_target(&args.target)?;
    args.conn.apply(&mut cfg);
    friki_tui::run(cfg).await
}

/// Verificador de claves de host sin diálogo: `known_hosts` acepta lo conocido y
/// `ask` rechaza siempre lo desconocido o cambiado (la CLI no pregunta).
fn rechazar_clave(
    host: &str,
    port: u16,
    key: &HostKey,
    estado: HostKeyStatus,
) -> Pin<Box<dyn Future<Output = HostKeyDecision> + Send>> {
    let motivo = if matches!(estado, HostKeyStatus::Changed { .. }) {
        "clave cambiada"
    } else {
        "clave desconocida"
    };
    eprintln!(
        "friki: {motivo} para {}:{port} ({})",
        sanitize_remote(host),
        key.fingerprint_sha256()
    );
    Box::pin(async { HostKeyDecision::Reject })
}

/// Pide secretos por terminal sin eco; sin TTY no pregunta nada.
struct TerminalPrompter {
    interactive: bool,
}

/// Lee una línea (con eco) o una contraseña (sin eco) fuera del runtime async.
/// El texto (que puede venir del servidor) se sanea antes de mostrarlo.
async fn ask(text: &str, echo: bool) -> Option<Zeroizing<String>> {
    let text = sanitize_remote(text);
    tokio::task::spawn_blocking(move || {
        if echo {
            eprint!("{text}");
            let mut line = Zeroizing::new(String::new());
            std::io::stdin().read_line(&mut line).ok()?;
            Some(Zeroizing::new(
                line.trim_end_matches(['\r', '\n']).to_string(),
            ))
        } else {
            rpassword::prompt_password(text).ok().map(Zeroizing::new)
        }
    })
    .await
    .ok()
    .flatten()
}

#[async_trait::async_trait]
impl AuthPrompter for TerminalPrompter {
    async fn password(&self, user: &str, host: &str) -> Option<Zeroizing<String>> {
        if !self.interactive {
            return None;
        }
        ask(&format!("{user}@{host}'s password: "), false).await
    }

    async fn passphrase(&self, key_path: &Path) -> Option<Zeroizing<String>> {
        if !self.interactive {
            return None;
        }
        ask(&format!("Passphrase for {}: ", key_path.display()), false).await
    }

    async fn keyboard_interactive(
        &self,
        name: &str,
        instructions: &str,
        prompts: &[(String, bool)],
    ) -> Option<Vec<Zeroizing<String>>> {
        if !self.interactive {
            return None;
        }
        if !name.is_empty() {
            eprintln!("{}", sanitize_remote(name));
        }
        if !instructions.is_empty() {
            eprintln!("{}", sanitize_remote(instructions));
        }
        let mut answers = Vec::with_capacity(prompts.len());
        for (text, echo) in prompts {
            answers.push(ask(text, *echo).await?);
        }
        Some(answers)
    }
}

/// Conecta sin TUI: `known_hosts` sin preguntar y secretos por terminal si la hay.
async fn connect(cfg: &SessionConfig) -> anyhow::Result<Connection> {
    let verifier = Arc::new(KnownHostsVerifier::new(
        paths::known_hosts_file(),
        rechazar_clave,
    ));
    let prompter = Arc::new(TerminalPrompter {
        interactive: std::io::stdin().is_terminal(),
    });
    let conn = Connection::connect(cfg, verifier, prompter).await?;
    for aviso in conn.warnings() {
        eprintln!("friki: aviso: {}", sanitize_remote(&aviso));
    }
    Ok(conn)
}

/// Sin shell interactivo no hay quien vigile el agente: se desactiva el reenvío, con aviso.
fn sin_agente(cfg: &mut SessionConfig) {
    if std::mem::take(&mut cfg.forward_agent) {
        eprintln!("friki: aviso: reenvío de agente desactivado en órdenes no interactivas");
    }
}

/// Resuelve el destino, aplica las opciones de conexión y conecta.
async fn connect_target(
    target: &str,
    conn: friki_cli::cli::ConnArgs,
) -> anyhow::Result<Connection> {
    let mut cfg = load_target(target)?;
    conn.apply(&mut cfg);
    sin_agente(&mut cfg);
    connect(&cfg).await
}

async fn exec(args: ExecArgs) -> anyhow::Result<i32> {
    let conn = connect_target(&args.target, args.conn).await?;
    let comando = args.command.join(" ");
    let (exit_status, exit_signal) = if args.json {
        let out = conn.exec(&comando).await?;
        let doc = serde_json::json!({
            "stdout": String::from_utf8_lossy(&out.stdout),
            "stderr": String::from_utf8_lossy(&out.stderr),
            "exit_status": out.exit_status,
            "exit_signal": out.exit_signal,
        });
        println!("{doc}");
        (out.exit_status, out.exit_signal)
    } else {
        // Con stdin de terminal no se reenvía nada: el remoto recibe EOF de inmediato.
        let mut stdout = tokio::io::stdout();
        let mut stderr = tokio::io::stderr();
        let st = if std::io::stdin().is_terminal() {
            conn.exec_streaming(&comando, tokio::io::empty(), &mut stdout, &mut stderr)
                .await?
        } else {
            conn.exec_streaming(&comando, tokio::io::stdin(), &mut stdout, &mut stderr)
                .await?
        };
        stdout.flush().await?;
        stderr.flush().await?;
        (st.exit_status, st.exit_signal)
    };
    let _ = conn.disconnect().await;
    codigo_remoto(exit_status, exit_signal.as_deref())
}

/// Código de salida de un comando remoto: su estado, o 128+N si lo mató la señal N.
/// Sin estado ni señal el servidor cerró sin decir nada: error de red (251).
fn codigo_remoto(exit_status: Option<u32>, exit_signal: Option<&str>) -> anyhow::Result<i32> {
    if let Some(signal) = exit_signal {
        return Ok(friki_ssh::exit_code_for_signal(signal));
    }
    match exit_status {
        // Los estados de salida de POSIX son de 8 bits.
        Some(status) => Ok((status & 0xff) as i32),
        None => Err(friki_ssh::Error::Network(
            "el servidor no envió estado de salida ni señal".into(),
        )
        .into()),
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
            let mut session = resolver_destino(&[], &target)?;
            session.name = name.clone();
            let path = config::save_session(&dir, &session)?;
            println!("Sesión '{name}' guardada en {}", path.display());
        }
        SessionsAction::ImportSshConfig { path, force } => {
            let path = match path {
                Some(p) => p,
                None => home_dir()?.join(".ssh").join("config"),
            };
            let (imported, avisos) = config::import_ssh_config_report(&path)?;
            for aviso in avisos {
                eprintln!("friki: aviso: {}", sanitize_remote(&aviso));
            }
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
            .expect("subcomando")
    }

    #[test]
    fn exit_code_red_es_251() {
        let e = anyhow::Error::from(friki_ssh::Error::Network("x".into()));
        assert_eq!(exit_code(&e), EXIT_NETWORK);
    }

    #[test]
    fn exit_code_auth_es_252() {
        let e = anyhow::Error::from(friki_ssh::Error::Auth("x".into()));
        assert_eq!(exit_code(&e), EXIT_AUTH);
    }

    #[test]
    fn exit_code_clave_de_host_es_253() {
        let e = anyhow::Error::from(friki_ssh::Error::HostKey("x".into()));
        assert_eq!(exit_code(&e), EXIT_HOST_KEY);
    }

    #[test]
    fn exit_code_negociacion_y_otros_son_254() {
        let neg = anyhow::Error::from(friki_ssh::Error::Negotiation("x".into()));
        let otro = anyhow::Error::from(friki_ssh::Error::Other("x".into()));
        assert_eq!(exit_code(&neg), EXIT_OTHER);
        assert_eq!(exit_code(&otro), EXIT_OTHER);
    }

    #[test]
    fn exit_code_error_de_core_es_254() {
        let e = anyhow::Error::from(friki_core::Error::Secret("x".into()));
        assert_eq!(exit_code(&e), EXIT_OTHER);
    }

    #[test]
    fn exit_code_error_ajeno_es_254() {
        assert_eq!(exit_code(&anyhow!("otro")), EXIT_OTHER);
    }

    #[test]
    fn exit_code_atraviesa_contexto() {
        let e = anyhow::Error::from(friki_ssh::Error::Auth("x".into())).context("conectando");
        assert_eq!(exit_code(&e), EXIT_AUTH);
    }

    #[test]
    fn error_de_uso_es_250() {
        let e = anyhow::Error::from(UsageError("destino no válido: host:abc".into()));
        assert_eq!(exit_code(&e), EXIT_USAGE);
    }

    #[test]
    fn destino_no_valido_es_error_de_uso() {
        let err = resolver_destino(&[], "host:abc").expect_err("puerto no numérico");
        assert_eq!(exit_code(&err), EXIT_USAGE);
    }

    #[test]
    fn sanitize_quita_escapes_y_controles() {
        assert_eq!(sanitize_remote("ok\x1b[2Jx"), "ok [2Jx");
        assert_eq!(sanitize_remote("a\rb\x07c\x00d"), "a b c d");
        assert_eq!(sanitize_remote("\x7f"), " ");
    }

    #[test]
    fn sanitize_quita_c1_y_respeta_texto_normal() {
        assert_eq!(sanitize_remote("\u{9b}31mrojo"), " 31mrojo");
        assert_eq!(sanitize_remote("Contraseña: ñ 日本"), "Contraseña: ñ 日本");
    }

    #[test]
    fn sanitize_sustituye_saltos_de_linea() {
        assert_eq!(sanitize_remote("Introduce\nPIN:"), "Introduce PIN:");
    }

    #[test]
    fn codigo_remoto_estado_y_senal() {
        assert_eq!(codigo_remoto(Some(0), None).ok(), Some(0));
        assert_eq!(codigo_remoto(Some(300), None).ok(), Some(44));
        assert_eq!(codigo_remoto(None, Some("KILL")).ok(), Some(137));
        assert_eq!(codigo_remoto(None, Some("SIGTERM")).ok(), Some(143));
    }

    #[test]
    fn codigo_remoto_sin_estado_ni_senal_es_red() {
        let e = codigo_remoto(None, None).expect_err("sin estado");
        assert_eq!(exit_code(&e), EXIT_NETWORK);
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

    fn local(bind: &str, port: u16, host: &str, host_port: u16) -> config::Forward {
        config::Forward::Local {
            bind: bind.into(),
            port,
            host: host.into(),
            host_port,
        }
    }

    #[test]
    fn sin_subcomando_abre_la_tui() {
        assert!(
            Cli::try_parse_from(["friki"])
                .expect("válido")
                .command
                .is_none()
        );
    }

    #[test]
    fn parsea_redirecciones_openssh() {
        use friki_cli::cli::{parse_dynamic, parse_local, parse_remote};
        assert_eq!(parse_local("8080:web:80"), Ok(local("", 8080, "web", 80)));
        assert_eq!(
            parse_local("*:8080:web:80"),
            Ok(local("*", 8080, "web", 80))
        );
        assert_eq!(
            parse_local("[::1]:8080:[fe80::1]:80"),
            Ok(local("::1", 8080, "fe80::1", 80))
        );
        assert_eq!(parse_local("8080:[::1]:80"), Ok(local("", 8080, "::1", 80)));
        assert!(parse_local("8080:web").is_err());
        assert!(parse_local("x:web:80").is_err());
        assert!(parse_local("8080::80").is_err());
        assert!(parse_local("[::1:8080:h:80").is_err());
        assert_eq!(
            parse_remote("0.0.0.0:2222:localhost:22"),
            Ok(config::Forward::Remote {
                bind: "0.0.0.0".into(),
                port: 2222,
                host: "localhost".into(),
                host_port: 22
            })
        );
        assert_eq!(
            parse_dynamic("1080"),
            Ok(config::Forward::Dynamic {
                bind: String::new(),
                port: 1080
            })
        );
        assert_eq!(
            parse_dynamic("[::1]:1080"),
            Ok(config::Forward::Dynamic {
                bind: "::1".into(),
                port: 1080
            })
        );
        assert!(parse_dynamic("a:b:1").is_err());
    }

    #[test]
    fn parsea_saltos() {
        use friki_cli::cli::parse_jump;
        assert!(parse_jump("a").is_ok());
        assert!(parse_jump("u@a:22,[::1]:2200,b").is_ok());
        assert!(parse_jump("").is_err());
        assert!(parse_jump("a,,b").is_err());
        assert!(parse_jump("a:x").is_err());
    }

    #[test]
    fn opciones_de_conexion_rellenan_la_sesion() {
        match parse(&[
            "friki",
            "fwd",
            "srv",
            "-J",
            "bast@jump:2200",
            "-L",
            "8080:web:80",
            "-R",
            "9000:localhost:9000",
            "-D",
            "1080",
            "-A",
            "--require-strict-kex",
        ]) {
            Command::Fwd(a) => {
                let mut cfg = SessionConfig::default();
                a.conn.apply(&mut cfg);
                assert_eq!(cfg.proxy_jump.as_deref(), Some("bast@jump:2200"));
                assert_eq!(cfg.forwards.len(), 3);
                assert_eq!(cfg.forwards[0], local("", 8080, "web", 80));
                assert!(cfg.forward_agent && cfg.require_strict_kex);
            }
            other => panic!("subcomando inesperado: {other:?}"),
        }
    }

    #[test]
    fn parsea_transferencias() {
        assert!(matches!(parse(&["friki", "ls", "h:/tmp"]), Command::Ls(_)));
        match parse(&["friki", "get", "-r", "h:/a", "/tmp/x"]) {
            Command::Get(a) => {
                assert_eq!(a.remote, "h:/a");
                assert_eq!(a.local, Some(PathBuf::from("/tmp/x")));
                assert!(a.recursive);
            }
            other => panic!("subcomando inesperado: {other:?}"),
        }
        match parse(&["friki", "put", "a", "b", "h:/d/"]) {
            Command::Put(a) => assert_eq!(a.paths, ["a", "b", "h:/d/"]),
            other => panic!("subcomando inesperado: {other:?}"),
        }
        assert!(Cli::try_parse_from(["friki", "put", "h:/d/"]).is_err());
        match parse(&["friki", "cp", "-r", "--scp", "h:/a", "."]) {
            Command::Cp(a) => assert!(a.scp && a.recursive),
            other => panic!("subcomando inesperado: {other:?}"),
        }
    }

    #[test]
    fn parsea_sync() {
        match parse(&[
            "friki",
            "sync",
            "dir",
            "h:/r",
            "--down",
            "--delete",
            "--include",
            "*.rs",
            "--exclude",
            "target/",
            "--dry-run",
            "--json",
        ]) {
            Command::Sync(a) => {
                assert!(a.down && !a.both && a.delete && a.dry_run && a.json);
                assert_eq!(a.include, ["*.rs"]);
                assert_eq!(a.exclude, ["target/"]);
            }
            other => panic!("subcomando inesperado: {other:?}"),
        }
        assert!(Cli::try_parse_from(["friki", "sync", "d", "h:/r", "--down", "--both"]).is_err());
    }

    #[test]
    fn parsea_ftp() {
        match parse(&["friki", "ftp", "ls", "h:2121", "--tls", "-u", "ana", "/pub"]) {
            Command::Ftp {
                action: friki_cli::cli::FtpAction::Ls { ftp, path, .. },
            } => {
                assert!(ftp.tls);
                assert_eq!((ftp.host.as_str(), ftp.user.as_str()), ("h:2121", "ana"));
                assert_eq!(path.as_deref(), Some("/pub"));
            }
            other => panic!("subcomando inesperado: {other:?}"),
        }
        assert!(matches!(
            parse(&["friki", "ftp", "get", "h", "/f"]),
            Command::Ftp {
                action: friki_cli::cli::FtpAction::Get { .. }
            }
        ));
        assert!(matches!(
            parse(&["friki", "ftp", "put", "h", "f"]),
            Command::Ftp {
                action: friki_cli::cli::FtpAction::Put { .. }
            }
        ));
    }

    #[test]
    fn parsea_pubkey_install_y_script() {
        match parse(&["friki", "pubkey-install", "h", "-i", "k.pub", "-I", "k"]) {
            Command::PubkeyInstall(a) => {
                assert_eq!(a.pubkey, Some(PathBuf::from("k.pub")));
                assert_eq!(a.auth_identity, [PathBuf::from("k")]);
            }
            other => panic!("subcomando inesperado: {other:?}"),
        }
        match parse(&[
            "friki",
            "script",
            "s.rhai",
            "--target",
            "h",
            "--allow-read",
            "/r",
            "--allow-write",
            "/w",
            "--allow-connect",
            "--",
            "-x",
            "y",
        ]) {
            Command::Script(a) => {
                assert_eq!(a.target.as_deref(), Some("h"));
                assert_eq!(a.allow_read, [PathBuf::from("/r")]);
                assert_eq!(a.allow_write, [PathBuf::from("/w")]);
                assert!(a.allow_connect);
                assert_eq!(a.args, ["-x", "y"]);
            }
            other => panic!("subcomando inesperado: {other:?}"),
        }
    }

    #[test]
    fn parsea_protocolos_de_la_tui() {
        match parse(&["friki", "serial", "list"]) {
            Command::Serial(a) => assert_eq!(a.device, "list"),
            other => panic!("subcomando inesperado: {other:?}"),
        }
        match parse(&[
            "friki",
            "serial",
            "/dev/ttyUSB0",
            "--baud",
            "9600",
            "--data-bits",
            "7",
            "--parity",
            "even",
            "--stop-bits",
            "2",
            "--flow",
            "hardware",
        ]) {
            Command::Serial(a) => {
                assert_eq!((a.baud, a.data_bits, a.stop_bits), (9600, 7, 2));
                assert_eq!(a.parity, config::Parity::Even);
                assert_eq!(a.flow, config::FlowControl::Hardware);
            }
            other => panic!("subcomando inesperado: {other:?}"),
        }
        assert!(Cli::try_parse_from(["friki", "serial", "d", "--data-bits", "9"]).is_err());
        match parse(&["friki", "telnet", "[::1]:2323", "--tls"]) {
            Command::Telnet(a) => assert!(a.tls && a.host == "[::1]:2323"),
            other => panic!("subcomando inesperado: {other:?}"),
        }
        match parse(&["friki", "shell", "ls", "-la"]) {
            Command::Shell(a) => assert_eq!(a.command, ["ls", "-la"]),
            other => panic!("subcomando inesperado: {other:?}"),
        }
    }

    #[test]
    fn errores_de_xfer_dan_codigos() {
        let nf = anyhow::Error::from(friki_xfer::Error::NotFound("x".into()));
        let pd = anyhow::Error::from(friki_xfer::Error::PermissionDenied("x".into()));
        let io = anyhow::Error::from(friki_xfer::Error::Io(std::io::ErrorKind::TimedOut.into()));
        let red = anyhow::Error::from(friki_xfer::Error::Ssh(friki_ssh::Error::Network(
            "x".into(),
        )));
        assert_eq!(exit_code(&nf), EXIT_OTHER);
        assert_eq!(exit_code(&pd), EXIT_OTHER);
        assert_eq!(exit_code(&io), EXIT_NETWORK);
        assert_eq!(exit_code(&red), EXIT_NETWORK);
        assert_eq!(failure_code(friki_xfer::ErrorKind::Network), EXIT_NETWORK);
        assert_eq!(
            failure_code(friki_xfer::ErrorKind::PermissionDenied),
            EXIT_OTHER
        );
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
                assert!(a.file.is_none() && a.comment == "FrikiTerm" && !a.no_passphrase);
            }
            other => panic!("subcomando inesperado: {other:?}"),
        }
    }
}
