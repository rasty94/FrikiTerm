//! Cola + SFTP real contra el OpenSSH de Docker (`eval "$(scripts/test-sshd.sh up)"`).
//! Sin `FRIKI_TEST_*` avisa y termina; con `FRIKI_TEST_REQUIRED=1` esa falta es un fallo.

use std::path::PathBuf;
use std::sync::Arc;

use friki_core::config::{AuthMethod, SessionConfig};
use friki_core::known_hosts::HostKey;
use friki_ssh::{AuthPrompter, Connection, HostKeyDecision, HostKeyVerifier, Zeroizing};
use friki_xfer::local::LocalFs;
use friki_xfer::queue::{Direction, Job, Queue, QueueEvent, QueueOptions, Resume};
use friki_xfer::{RemoteFs, sftp};
use tokio::sync::mpsc::unbounded_channel;

struct AcceptAll;

#[async_trait::async_trait]
impl HostKeyVerifier for AcceptAll {
    async fn verify(&self, _: &str, _: u16, _: &HostKey) -> HostKeyDecision {
        HostKeyDecision::AcceptOnce
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

fn required() -> bool {
    std::env::var("FRIKI_TEST_REQUIRED").is_ok_and(|v| v == "1")
}

async fn connect() -> Option<Connection> {
    let v = |n: &str| std::env::var(n).ok().filter(|s| !s.is_empty());
    let cfg = (|| {
        Some(SessionConfig {
            host: v("FRIKI_TEST_HOST")?,
            port: v("FRIKI_TEST_PORT")?.parse().ok()?,
            user: Some(v("FRIKI_TEST_USER")?),
            auth: vec![AuthMethod::PublicKey],
            identity_files: vec![PathBuf::from(v("FRIKI_TEST_KEY")?)],
            connect_timeout_secs: 10,
            ..Default::default()
        })
    })();
    let Some(cfg) = cfg else {
        assert!(
            !required(),
            "FRIKI_TEST_REQUIRED=1 y faltan variables FRIKI_TEST_*: scripts/test-sshd.sh up"
        );
        eprintln!("SALTADA (no ejecutada): faltan variables FRIKI_TEST_*");
        return None;
    };
    Some(
        Connection::connect(&cfg, Arc::new(AcceptAll), Arc::new(NoPrompt))
            .await
            .expect("conexión SSH"),
    )
}

fn job(direction: Direction, src: &str, dst: &str) -> Job {
    Job {
        direction,
        src: src.into(),
        dst: dst.into(),
        recursive: true,
        resume: Resume::Auto,
        preserve_mtime: false,
    }
}

#[tokio::test]
#[ignore = "requiere OpenSSH de pruebas"]
async fn cola_sube_y_baja_directorio_por_sftp() {
    let Some(conn) = connect().await else { return };
    let remote: Arc<dyn RemoteFs> = Arc::from(sftp::open(&conn).await.expect("sftp"));
    let user = std::env::var("FRIKI_TEST_USER").expect("user");
    assert!(
        remote.label().starts_with(&format!("sftp://{user}@")),
        "{}",
        remote.label()
    );

    let tmp = tempfile::tempdir().expect("tmp");
    let big: Vec<u8> = (0..150_000u32).map(|i| (i % 247) as u8).collect();
    std::fs::create_dir_all(tmp.path().join("d/sub")).expect("dirs");
    std::fs::write(tmp.path().join("d/a.txt"), "hola").expect("a");
    std::fs::write(tmp.path().join("d/b.bin"), &big).expect("b");
    std::fs::write(tmp.path().join("d/sub/c.txt"), "adiós").expect("c");

    let rdir = format!("/tmp/friki-queue-sftp-{}", std::process::id());
    let (tx, mut rx) = unbounded_channel();
    let q = Queue::new(
        Arc::new(LocalFs::new(tmp.path())),
        remote.clone(),
        QueueOptions::default(),
        tx,
    );
    q.push(job(Direction::Upload, "d", &rdir));
    assert_eq!(q.wait().await, (1, 0));
    q.push(job(Direction::Download, &rdir, "back"));
    assert_eq!(q.wait().await, (2, 0));

    let finished: Vec<u64> = std::iter::from_fn(|| rx.try_recv().ok())
        .filter_map(|e| match e {
            QueueEvent::Finished { bytes, .. } => Some(bytes),
            _ => None,
        })
        .collect();
    assert_eq!(finished, vec![150_010, 150_010]); // 4 + 150_000 + 6 ("adiós" = 6 bytes)
    for f in ["a.txt", "b.bin", "sub/c.txt"] {
        assert_eq!(
            std::fs::read(tmp.path().join("d").join(f)).expect("orig"),
            std::fs::read(tmp.path().join("back").join(f)).expect("copia"),
            "{f}"
        );
    }

    conn.exec(&format!("rm -rf '{rdir}'"))
        .await
        .expect("limpieza");
    conn.disconnect().await.expect("disconnect");
}
