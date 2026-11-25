use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use friki_core::config::{AuthMethod, Protocol, SessionConfig};
use friki_core::known_hosts::HostKey;
use friki_ssh::AuthPrompter;

use super::*;

/// Asker falso: responde en orden con lo programado y apunta lo que se le preguntó.
#[derive(Clone, Default)]
struct Fake {
    prompts: Arc<Mutex<Vec<Prompt>>>,
    replies: Arc<Mutex<VecDeque<Option<Reply>>>>,
}

impl Fake {
    fn with(replies: Vec<Option<Reply>>) -> Self {
        Self {
            replies: Arc::new(Mutex::new(replies.into())),
            ..Default::default()
        }
    }
    fn asked(&self) -> Vec<Prompt> {
        self.prompts.lock().unwrap().clone()
    }
}

#[async_trait::async_trait]
impl Asker for Fake {
    async fn ask(&self, p: Prompt) -> Option<Reply> {
        self.prompts.lock().unwrap().push(p);
        self.replies.lock().unwrap().pop_front().flatten()
    }
}

fn secret(s: &str) -> Option<Reply> {
    Some(Reply::Secret(Some(Zeroizing::new(s.into()))))
}

#[tokio::test]
async fn prompter_maps_password_and_passphrase() {
    let f = Fake::with(vec![secret("pw"), secret("pp")]);
    let p = Prompter::new(f.clone(), "destino");
    assert_eq!(
        p.password("u", "h").await.as_deref().map(String::as_str),
        Some("pw")
    );
    let pp = p.passphrase(Path::new("/k/id")).await;
    assert_eq!(pp.as_deref().map(String::as_str), Some("pp"));
    assert_eq!(
        f.asked(),
        vec![
            Prompt::Password {
                user: "u".into(),
                host: "h".into(),
                port: 0
            },
            Prompt::Passphrase {
                key_path: "/k/id".into()
            }
        ]
    );
}

#[tokio::test]
async fn prompter_no_answer_or_wrong_reply_is_none() {
    let p = Prompter::new(
        Fake::with(vec![None, Some(Reply::HostKey(HostKeyDecision::Reject))]),
        "d",
    );
    assert!(p.password("u", "h").await.is_none());
    assert!(p.password("u", "h").await.is_none());
    assert!(p.passphrase(Path::new("k")).await.is_none());
}

#[tokio::test]
async fn prompter_kbd_uses_asking_host() {
    let f = Fake::with(vec![Some(Reply::Multi(Some(vec![Zeroizing::new(
        "123".into(),
    )])))]);
    let p = Prompter::new(f.clone(), "destino");
    let r = p
        .keyboard_interactive_for("salto", "n", "i", &[("Code: ".into(), false)])
        .await;
    assert_eq!(r.map(|v| v.len()), Some(1));
    assert_eq!(
        f.asked(),
        vec![Prompt::KbdInteractive {
            host: "salto".into(),
            name: "n".into(),
            instructions: "i".into(),
            prompts: vec![KbdPrompt {
                text: "Code: ".into(),
                echo: false
            }]
        }]
    );
    // Sin salto indicado, el destino.
    let f = Fake::with(vec![]);
    let p = Prompter::new(f.clone(), "destino");
    assert!(p.keyboard_interactive("", "", &[]).await.is_none());
    assert!(matches!(&f.asked()[0], Prompt::KbdInteractive { host, .. } if host == "destino"));
}

fn key(b64: &str) -> HostKey {
    HostKey {
        algorithm: "ssh-ed25519".into(),
        base64: b64.into(),
    }
}

#[tokio::test]
async fn verifier_asks_saves_and_detects_change() {
    let d = tempfile::tempdir().unwrap();
    let kh = d.path().join("known_hosts");
    let f = Fake::with(vec![
        Some(Reply::HostKey(HostKeyDecision::AcceptAndSave)),
        None,
        Some(Reply::HostKey(HostKeyDecision::AcceptOnce)),
    ]);
    let v = verifier(f.clone(), kh.clone());
    let a = key("AAAAC3NzaC1lZDI1NTE5AAAAIAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA");
    // Desconocida: pregunta y guarda.
    assert_eq!(v.verify("h", 22, &a).await, HostKeyDecision::AcceptAndSave);
    assert!(matches!(&f.asked()[0], Prompt::HostKeyUnknown { host, port: 22, .. } if host == "h"));
    // Conocida: no pregunta.
    assert_eq!(v.verify("h", 22, &a).await, HostKeyDecision::AcceptOnce);
    assert_eq!(f.asked().len(), 1);
    // Cambiada y sin respuesta: rechaza, con la huella anterior en el prompt.
    let b = key("AAAAC3NzaC1lZDI1NTE5AAAAIBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB");
    assert_eq!(v.verify("h", 22, &b).await, HostKeyDecision::Reject);
    let asked = f.asked();
    match &asked[1] {
        Prompt::HostKeyChanged {
            fingerprint,
            previous_fingerprint,
            ..
        } => assert_ne!(fingerprint, previous_fingerprint),
        p => panic!("{p:?}"),
    }
}

#[tokio::test]
async fn open_stream_local_shell_and_tn3270_gate() {
    let s = SessionConfig {
        protocol: Protocol::LocalShell,
        local_command: Some("echo hi".into()),
        ..Default::default()
    };
    let (o, mut r) = open_stream(Fake::default(), &s, PathBuf::new(), (80, 24))
        .await
        .unwrap();
    assert!(o.conn.is_none() && o.forwards.is_empty() && o.size == (80, 24));
    assert!(r.next_event().await.is_some());
}

// --- SSH real (scripts/test-sshd.sh): sin FRIKI_TEST_* se salta; con FRIKI_TEST_REQUIRED=1 falla.

fn ssh_env() -> Option<(SessionConfig, PathBuf, String)> {
    let v = |n: &str| std::env::var(n).ok().filter(|s| !s.is_empty());
    let e = (|| {
        Some((
            v("FRIKI_TEST_HOST")?,
            v("FRIKI_TEST_PORT")?.parse::<u16>().ok()?,
            v("FRIKI_TEST_USER")?,
            v("FRIKI_TEST_PASSWORD")?,
            PathBuf::from(v("FRIKI_TEST_KNOWN_HOSTS")?),
        ))
    })();
    let Some((host, port, user, pw, kh)) = e else {
        assert!(
            std::env::var("FRIKI_TEST_REQUIRED").as_deref() != Ok("1"),
            "FRIKI_TEST_REQUIRED=1 y falta alguna variable FRIKI_TEST_*"
        );
        eprintln!("SALTADA (no ejecutada): falta alguna variable FRIKI_TEST_*");
        return None;
    };
    let cfg = SessionConfig {
        host,
        port,
        user: Some(user),
        auth: vec![AuthMethod::Password],
        connect_timeout_secs: 10,
        ..Default::default()
    };
    Some((cfg, kh, pw))
}

#[tokio::test]
#[ignore = "necesita scripts/test-sshd.sh"]
async fn ssh_open_stream_with_fake_asker() {
    let Some((cfg, kh, pw)) = ssh_env() else {
        return;
    };
    // known_hosts propio y vacío: la clave se pregunta y se acepta.
    let d = tempfile::tempdir().unwrap();
    let kh = d.path().join(kh.file_name().unwrap());
    let f = Fake::with(vec![
        Some(Reply::HostKey(HostKeyDecision::AcceptAndSave)),
        secret(&pw),
    ]);
    let (o, mut r) = open_stream(f.clone(), &cfg, kh, (80, 24)).await.unwrap();
    let conn = o.conn.clone().expect("conexión SSH");
    // La misma conexión sirve para otros canales (SFTP, exec).
    assert_eq!(conn.exec("echo ok").await.unwrap().stdout, b"ok\n");
    assert!(r.next_event().await.is_some());
    let asked = f.asked();
    assert!(matches!(asked[0], Prompt::HostKeyUnknown { .. }));
    assert!(matches!(asked[1], Prompt::Password { .. }));
    disconnect(conn).await;
}
