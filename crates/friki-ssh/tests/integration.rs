//! Pruebas contra un OpenSSH real (`scripts/test-sshd.sh`). Marcadas `#[ignore]`;
//! se saltan limpiamente si faltan las variables `FRIKI_TEST_*`.
//! Ejecutar con: `cargo test -p friki-ssh -- --ignored`.

use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;

use friki_core::config::{AuthMethod, SessionConfig};
use friki_core::known_hosts::{HostKey, HostKeyStatus};
use friki_ssh::{
    AuthPrompter, Connection, Error, HostKeyDecision, HostKeyVerifier, KnownHostsVerifier,
    ShellEvent,
};

struct Env {
    host: String,
    port: u16,
    legacy_port: Option<u16>,
    user: String,
    password: String,
    key: PathBuf,
    known_hosts: PathBuf,
}

fn env() -> Option<Env> {
    let v = |n: &str| std::env::var(n).ok().filter(|s| !s.is_empty());
    let e = (|| {
        Some(Env {
            host: v("FRIKI_TEST_HOST")?,
            port: v("FRIKI_TEST_PORT")?.parse().ok()?,
            legacy_port: v("FRIKI_TEST_LEGACY_PORT")
                .and_then(|p| p.parse().ok())
                .or(Some(2223)),
            user: v("FRIKI_TEST_USER")?,
            password: v("FRIKI_TEST_PASSWORD")?,
            key: v("FRIKI_TEST_KEY")?.into(),
            known_hosts: v("FRIKI_TEST_KNOWN_HOSTS")?.into(),
        })
    })();
    if e.is_none() {
        eprintln!("saltada: faltan variables FRIKI_TEST_*");
    }
    e
}

fn cfg(e: &Env, auth: Vec<AuthMethod>) -> SessionConfig {
    SessionConfig {
        host: e.host.clone(),
        port: e.port,
        user: Some(e.user.clone()),
        auth,
        identity_files: vec![e.key.clone()],
        connect_timeout_secs: 10,
        ..Default::default()
    }
}

struct AcceptAll;

#[async_trait::async_trait]
impl HostKeyVerifier for AcceptAll {
    async fn verify(&self, _: &str, _: u16, _: &HostKey) -> HostKeyDecision {
        HostKeyDecision::AcceptOnce
    }
}

struct Prompter(String);

#[async_trait::async_trait]
impl AuthPrompter for Prompter {
    async fn password(&self, _: &str, _: &str) -> Option<String> {
        Some(self.0.clone())
    }
    async fn passphrase(&self, _: &Path) -> Option<String> {
        None
    }
    async fn keyboard_interactive(
        &self,
        _: &str,
        _: &str,
        _: &[(String, bool)],
    ) -> Option<Vec<String>> {
        None
    }
}

type Ask = Pin<Box<dyn Future<Output = HostKeyDecision> + Send>>;

#[tokio::test]
#[ignore = "requiere OpenSSH de pruebas"]
async fn conecta_con_password_y_ejecuta_echo() {
    let Some(e) = env() else { return };
    let c = cfg(&e, vec![AuthMethod::Password]);
    let conn = Connection::connect(
        &c,
        Arc::new(AcceptAll),
        Arc::new(Prompter(e.password.clone())),
    )
    .await
    .expect("conexión con contraseña");
    let out = conn.exec("echo hola-friki").await.expect("exec");
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "hola-friki");
    assert_eq!(out.exit_status, Some(0));
    conn.disconnect().await.expect("disconnect");
}

#[tokio::test]
#[ignore = "requiere OpenSSH de pruebas"]
async fn conecta_con_clave() {
    let Some(e) = env() else { return };
    // Contraseña errónea a propósito: solo puede entrar por la clave.
    let c = cfg(&e, vec![AuthMethod::PublicKey]);
    let conn = Connection::connect(&c, Arc::new(AcceptAll), Arc::new(Prompter("mal".into())))
        .await
        .expect("conexión con clave");
    let out = conn.exec("echo clave-ok").await.expect("exec");
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "clave-ok");
}

#[tokio::test]
#[ignore = "requiere OpenSSH de pruebas"]
async fn contrasena_mala_da_error_de_autenticacion() {
    let Some(e) = env() else { return };
    let c = cfg(&e, vec![AuthMethod::Password]);
    let r = Connection::connect(&c, Arc::new(AcceptAll), Arc::new(Prompter("mal".into()))).await;
    assert!(matches!(r, Err(Error::Auth(_))), "{:?}", r.err());
}

#[tokio::test]
#[ignore = "requiere OpenSSH de pruebas"]
async fn abre_shell_y_ejecuta() {
    let Some(e) = env() else { return };
    let c = cfg(&e, vec![AuthMethod::PublicKey]);
    let conn = Connection::connect(&c, Arc::new(AcceptAll), Arc::new(Prompter(String::new())))
        .await
        .expect("conexión");
    let (w, mut r) = conn
        .open_shell("xterm-256color", 80, 24)
        .await
        .expect("shell");
    w.resize(100, 30).await.expect("resize");
    w.write(b"echo SHELL-$((40+2))\nexit 3\n")
        .await
        .expect("write");
    let (mut texto, mut salida, mut cerrado) = (Vec::new(), None, false);
    let leer = async {
        while let Some(ev) = r.next_event().await {
            match ev {
                ShellEvent::Data(d) => texto.extend(d),
                ShellEvent::ExitStatus(n) => salida = Some(n),
                ShellEvent::Closed => {
                    cerrado = true;
                    break;
                }
            }
        }
    };
    tokio::time::timeout(std::time::Duration::from_secs(15), leer)
        .await
        .expect("el shell no terminó");
    assert!(String::from_utf8_lossy(&texto).contains("SHELL-42"));
    assert_eq!(salida, Some(3));
    assert!(cerrado);
}

#[tokio::test]
#[ignore = "requiere OpenSSH de pruebas"]
async fn clave_de_host_cambiada_falla() {
    let Some(e) = env() else { return };
    // known_hosts con otra clave Ed25519 para el mismo host.
    let dir = tempfile::tempdir().expect("tempdir");
    let otra =
        friki_ssh::keys::generate_ed25519(&dir.path().join("otra"), "x", None).expect("keygen");
    let otra_b64 = otra.split_whitespace().nth(1).expect("base64").to_string();
    let original = std::fs::read_to_string(&e.known_hosts).expect("known_hosts");
    let falsificado: String = original
        .lines()
        .filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
        .filter_map(|l| {
            let mut p: Vec<&str> = l.split_whitespace().collect();
            (p.len() >= 3 && p[1] == "ssh-ed25519").then(|| {
                p[2] = &otra_b64;
                p.join(" ")
            })
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        !falsificado.is_empty(),
        "el known_hosts de pruebas no tiene clave ed25519"
    );
    let kh = dir.path().join("known_hosts");
    std::fs::write(&kh, format!("{falsificado}\n")).expect("escribir");

    let verifier = KnownHostsVerifier {
        path: kh,
        ask: |_: &str, _: u16, _: &HostKey, status: HostKeyStatus| -> Ask {
            Box::pin(async move {
                assert!(
                    matches!(status, HostKeyStatus::Changed { .. }),
                    "estado: {status:?}"
                );
                HostKeyDecision::Reject
            })
        },
    };
    let c = cfg(&e, vec![AuthMethod::PublicKey]);
    let r = Connection::connect(&c, Arc::new(verifier), Arc::new(Prompter(String::new()))).await;
    assert!(matches!(r, Err(Error::HostKey(_))), "{:?}", r.err());
}

#[tokio::test]
#[ignore = "requiere OpenSSH de pruebas"]
async fn clave_de_host_conocida_conecta_sin_preguntar() {
    let Some(e) = env() else { return };
    let verifier = KnownHostsVerifier {
        path: e.known_hosts.clone(),
        ask: |_: &str, _: u16, _: &HostKey, _: HostKeyStatus| -> Ask {
            Box::pin(async { panic!("no debía preguntar: la clave es conocida") })
        },
    };
    let c = cfg(&e, vec![AuthMethod::PublicKey]);
    Connection::connect(&c, Arc::new(verifier), Arc::new(Prompter(String::new())))
        .await
        .expect("conexión con host conocido");
}

#[tokio::test]
#[ignore = "requiere OpenSSH de pruebas"]
async fn politica_segura_no_negocia_con_servidor_heredado() {
    let Some(e) = env() else { return };
    let Some(port) = e.legacy_port else { return };
    let mut c = cfg(&e, vec![AuthMethod::Password]);
    c.port = port;
    let r = Connection::connect(
        &c,
        Arc::new(AcceptAll),
        Arc::new(Prompter(e.password.clone())),
    )
    .await;
    assert!(matches!(r, Err(Error::Negotiation(_))), "{:?}", r.err());
}
