//! Lógica de sesión de la TUI contra un OpenSSH real, sin terminal: `TestBackend` y
//! teclas sintéticas. `scripts/test-sshd.sh up` exporta las variables `FRIKI_TEST_*`.
//! Sin ellas cada prueba lo dice por stderr y termina; con `FRIKI_TEST_REQUIRED=1`
//! esa falta es un fallo (para CI).
//!
//! `cargo test -p friki-tui -- --include-ignored`

use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;

use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use friki_core::config::{AuthMethod, SessionConfig};
use friki_core::known_hosts::{HostKey, HostKeyStatus};
use friki_ssh::{AuthPrompter, Connection, HostKeyDecision, KnownHostsVerifier, Zeroizing};
use friki_tui::{Connected, Paths, connect_ui, shell_session};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use tokio::sync::mpsc;

struct Env {
    host: String,
    port: u16,
    user: String,
    key: PathBuf,
    known_hosts: PathBuf,
}

fn required() -> bool {
    std::env::var("FRIKI_TEST_REQUIRED").is_ok_and(|v| v == "1")
}

fn env() -> Option<Env> {
    let v = |n: &str| std::env::var(n).ok().filter(|s| !s.is_empty());
    let e = (|| {
        Some(Env {
            host: v("FRIKI_TEST_HOST")?,
            port: v("FRIKI_TEST_PORT")?.parse().ok()?,
            user: v("FRIKI_TEST_USER")?,
            key: v("FRIKI_TEST_KEY")?.into(),
            known_hosts: v("FRIKI_TEST_KNOWN_HOSTS")?.into(),
        })
    })();
    if e.is_none() {
        assert!(
            !required(),
            "FRIKI_TEST_REQUIRED=1 y falta alguna variable FRIKI_TEST_*: scripts/test-sshd.sh up"
        );
        eprintln!("SALTADA (no ejecutada): falta alguna variable FRIKI_TEST_*");
    }
    e
}

fn session(e: &Env) -> SessionConfig {
    SessionConfig {
        name: "prueba".into(),
        host: e.host.clone(),
        port: e.port,
        user: Some(e.user.clone()),
        auth: vec![AuthMethod::PublicKey],
        identity_files: vec![e.key.clone()],
        connect_timeout_secs: 10,
        term: "xterm-256color".into(),
        ..Default::default()
    }
}

struct NoPrompt;

#[async_trait::async_trait]
impl AuthPrompter for NoPrompt {
    async fn password(&self, _: &str, _: &str) -> Option<Zeroizing<String>> {
        None
    }
    async fn passphrase(&self, _: &std::path::Path) -> Option<Zeroizing<String>> {
        None
    }
    async fn keyboard_interactive(
        &self,
        _: &str,
        _: &str,
        _: &[(String, bool)],
    ) -> Option<Vec<Zeroizing<String>>> {
        None
    }
}

/// Deja la clave del servidor en known_hosts para que la TUI no pregunte (no hay
/// quien conteste al diálogo en una prueba sin teclado).
async fn trust_host(e: &Env, s: &SessionConfig) {
    let v = Arc::new(KnownHostsVerifier::new(
        e.known_hosts.clone(),
        |_: &str, _: u16, _: &HostKey, _: HostKeyStatus| {
            Box::pin(async { HostKeyDecision::AcceptAndSave })
                as Pin<Box<dyn Future<Output = HostKeyDecision> + Send>>
        },
    ));
    Connection::connect(s, v, Arc::new(NoPrompt))
        .await
        .expect("conexión previa")
        .disconnect()
        .await
        .expect("desconexión");
}

fn key(c: KeyCode) -> Event {
    Event::Key(KeyEvent::new(c, KeyModifiers::NONE))
}

fn typed(s: &str) -> Vec<Event> {
    s.chars()
        .map(|c| {
            key(if c == '\n' {
                KeyCode::Enter
            } else {
                KeyCode::Char(c)
            })
        })
        .collect()
}

/// Ejecuta la TUI con `events` ya encolados y devuelve el código y el texto final.
async fn drive(e: &Env, events: Vec<Event>) -> (anyhow::Result<i32>, String) {
    let s = session(e);
    trust_host(e, &s).await;
    let paths = Paths {
        known_hosts: e.known_hosts.clone(),
        logs_dir: std::env::temp_dir().join(format!("friki-tui-test-{}", std::process::id())),
    };
    let mut tui = Terminal::new(TestBackend::new(100, 25)).expect("terminal");
    let (tx, rx) = mpsc::unbounded_channel::<std::io::Result<Event>>();
    let mut stream = Box::pin(futures_util::stream::unfold(rx, |mut rx| async move {
        rx.recv().await.map(|ev| (ev, rx))
    }));
    // Conexión: sin teclas (la clave es conocida y la autenticación es por clave).
    let conn = match connect_ui(&mut tui, &mut stream, &s, &paths)
        .await
        .expect("connect_ui")
    {
        Connected::Ready(c) => c,
        _ => panic!("la conexión no llegó a estar lista"),
    };
    for ev in events {
        tx.send(Ok(ev)).expect("evento");
    }
    let res = shell_session(&mut tui, &mut stream, conn, &s, &paths).await;
    let text: String = tui
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|c| c.symbol())
        .collect();
    (res, text)
}

#[tokio::test]
#[ignore = "requiere OpenSSH de pruebas"]
async fn shell_ejecuta_y_devuelve_el_codigo_de_salida() {
    let Some(e) = env() else { return };
    let mut evs = typed("echo friki-$((40+2))\n");
    evs.extend(typed("exit 7\n"));
    let (res, text) = drive(&e, evs).await;
    assert_eq!(res.expect("sesión"), 7);
    assert!(text.contains("friki-42"), "pantalla: {text}");
    assert!(text.contains("prueba"), "barra de estado: {text}");
}

#[tokio::test]
#[ignore = "requiere OpenSSH de pruebas"]
async fn ctrl_corchete_q_cierra_la_sesion() {
    let Some(e) = env() else { return };
    let mut evs = typed("echo listo\n");
    evs.push(Event::Key(KeyEvent::new(
        KeyCode::Char(']'),
        KeyModifiers::CONTROL,
    )));
    evs.push(key(KeyCode::Char('q')));
    let (res, _) = drive(&e, evs).await;
    assert_eq!(res.expect("sesión"), 0);
}
