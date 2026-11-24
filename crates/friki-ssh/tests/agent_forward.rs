//! `forward_agent` contra el OpenSSH de pruebas. Va en su propio binario porque fija
//! `SSH_AUTH_SOCK` (variable de entorno del proceso) antes de lanzar ninguna tarea.

#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use friki_core::config::{AuthMethod, SessionConfig};
use friki_core::known_hosts::HostKey;
use friki_core::transport::StreamEvent;
use friki_ssh::{AuthPrompter, Connection, HostKeyDecision, HostKeyVerifier, Zeroizing};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

struct Yes;

#[async_trait::async_trait]
impl HostKeyVerifier for Yes {
    async fn verify(&self, _: &str, _: u16, _: &HostKey) -> HostKeyDecision {
        HostKeyDecision::AcceptOnce
    }
}

#[async_trait::async_trait]
impl AuthPrompter for Yes {
    async fn password(&self, _: &str, _: &str) -> Option<Zeroizing<String>> {
        None
    }
    async fn passphrase(&self, _: &Path) -> Option<Zeroizing<String>> {
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

fn var(n: &str) -> Option<String> {
    std::env::var(n).ok().filter(|s| !s.is_empty())
}

/// Agente falso: responde a `SSH_AGENTC_REQUEST_IDENTITIES` con cero identidades.
async fn fake_agent(listener: tokio::net::UnixListener) {
    while let Ok((mut s, _)) = listener.accept().await {
        tokio::spawn(async move {
            let mut buf = [0u8; 5];
            while s.read_exact(&mut buf).await.is_ok() {
                // longitud 1, tipo 11 -> IDENTITIES_ANSWER (12) con 0 claves.
                if s.write_all(&[0, 0, 0, 5, 12, 0, 0, 0, 0]).await.is_err() {
                    break;
                }
            }
        });
    }
}

#[tokio::test]
#[ignore = "requiere OpenSSH de pruebas"]
async fn el_servidor_usa_el_agente_local() {
    let (Some(host), Some(port), Some(user), Some(key)) = (
        var("FRIKI_TEST_HOST"),
        var("FRIKI_TEST_PORT").and_then(|p| p.parse().ok()),
        var("FRIKI_TEST_USER"),
        var("FRIKI_TEST_KEY"),
    ) else {
        assert!(
            var("FRIKI_TEST_REQUIRED").as_deref() != Some("1"),
            "FRIKI_TEST_REQUIRED=1 y faltan variables FRIKI_TEST_*"
        );
        eprintln!("SALTADA (no ejecutada): faltan variables FRIKI_TEST_*");
        return;
    };

    let dir = tempfile::tempdir().expect("tempdir");
    let sock: PathBuf = dir.path().join("agent.sock");
    let listener = tokio::net::UnixListener::bind(&sock).expect("bind agente");
    tokio::spawn(fake_agent(listener));
    // SAFETY: único test del binario y ninguna otra tarea lee el entorno todavía.
    unsafe { std::env::set_var("SSH_AUTH_SOCK", &sock) };

    let cfg = SessionConfig {
        host,
        port,
        user: Some(user),
        auth: vec![AuthMethod::PublicKey],
        identity_files: vec![key.into()],
        connect_timeout_secs: 10,
        forward_agent: true,
        ..Default::default()
    };
    let conn = Connection::connect(&cfg, Arc::new(Yes), Arc::new(Yes))
        .await
        .expect("conexión");
    assert!(
        conn.warnings().iter().any(|w| w.contains("agente")),
        "{:?}",
        conn.warnings()
    );
    let (w, mut r) = conn
        .open_shell_stream("xterm", 80, 24)
        .await
        .expect("shell");
    w.write(b"ssh-add -l; exit\n").await.expect("write");
    let mut text = Vec::new();
    let run = async {
        while let Some(ev) = r.next_event().await {
            if let StreamEvent::Data(d) = ev {
                text.extend(d);
            }
        }
    };
    tokio::time::timeout(Duration::from_secs(10), run)
        .await
        .expect("plazo");
    let text = String::from_utf8_lossy(&text);
    assert!(text.contains("no identities"), "salida: {text}");
}
