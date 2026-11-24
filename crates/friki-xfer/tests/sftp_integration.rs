//! SFTP contra el OpenSSH de Docker (`eval "$(scripts/test-sshd.sh up)"`):
//! `cargo test -p friki-xfer -- --include-ignored`. Sin `FRIKI_TEST_*` la prueba avisa y
//! termina; con `FRIKI_TEST_REQUIRED=1` esa falta es un fallo.

use std::path::PathBuf;
use std::sync::Arc;

use friki_core::config::{AuthMethod, SessionConfig};
use friki_core::known_hosts::HostKey;
use friki_ssh::{AuthPrompter, Connection, HostKeyDecision, HostKeyVerifier, Zeroizing};
use friki_xfer::{Error, Kind, sftp};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

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

#[tokio::test]
#[ignore = "requiere OpenSSH de pruebas"]
async fn sftp_contra_sshd() {
    let Some(conn) = connect().await else { return };
    let fs = sftp::open(&conn).await.expect("sftp");
    let base = format!("/tmp/friki-xfer-sftp-{}", std::process::id());
    let p = |s: &str| format!("{base}/{s}");
    fs.mkdir(&base).await.expect("mkdir");

    let data: Vec<u8> = (0..200_000u32).map(|i| (i % 253) as u8).collect();
    // Subida cortada a medias y reanudada.
    let mut w = fs.open_write(&p("f.bin"), 0).await.expect("w0");
    w.write_all(&data[..80_000]).await.expect("w");
    w.shutdown().await.expect("shutdown");
    let mut w = fs.open_write(&p("f.bin"), 80_000).await.expect("w1");
    w.write_all(&data[80_000..]).await.expect("w");
    w.shutdown().await.expect("shutdown");

    let l = fs.list(&base).await.expect("list");
    assert_eq!(l.len(), 1);
    assert_eq!((l[0].kind, l[0].size), (Kind::File, 200_000));
    assert!(l[0].mode.is_some_and(|m| m & 0o400 != 0));

    // Bajada completa y desde la mitad.
    let mut r = fs.open_read(&p("f.bin"), 0).await.expect("r");
    let mut got = Vec::new();
    r.read_to_end(&mut got).await.expect("read");
    assert_eq!(got, data);
    let mut r = fs.open_read(&p("f.bin"), 100_000).await.expect("r");
    let mut got = Vec::new();
    r.read_to_end(&mut got).await.expect("read");
    assert_eq!(got, data[100_000..]);

    fs.rename(&p("f.bin"), &p("g.bin")).await.expect("rename");
    assert!(fs.stat(&p("f.bin")).await.expect("stat").is_none());
    assert!(fs.stat(&p("g.bin")).await.expect("stat").is_some());
    fs.set_mtime(&p("g.bin"), 1_700_000_000)
        .await
        .expect("mtime");

    // Errores mapeados.
    let r = fs.open_read(&p("no-existe"), 0).await;
    assert!(matches!(r, Err(Error::NotFound(_))), "{:?}", r.err());
    let r = fs.mkdir("/proc/friki").await;
    assert!(
        matches!(r, Err(Error::PermissionDenied(_) | Error::Protocol(_))),
        "{:?}",
        r.err()
    );

    fs.remove_file(&p("g.bin")).await.expect("rm");
    fs.remove_dir(&base).await.expect("rmdir");
    conn.disconnect().await.expect("disconnect");
}
