//! SCP contra el OpenSSH de Docker (`eval "$(scripts/test-sshd.sh up)"`):
//! `cargo test -p friki-xfer -- --include-ignored`. Sin `FRIKI_TEST_*` la prueba avisa y
//! termina; con `FRIKI_TEST_REQUIRED=1` esa falta es un fallo.

use std::path::PathBuf;
use std::sync::Arc;

use friki_core::config::{AuthMethod, SessionConfig};
use friki_core::known_hosts::HostKey;
use friki_ssh::{AuthPrompter, Connection, HostKeyDecision, HostKeyVerifier, Zeroizing};
use friki_xfer::scp;

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
async fn scp_fichero_y_recursivo() {
    let Some(conn) = connect().await else { return };
    let tmp = tempfile::tempdir().expect("tmp");
    let src = tmp.path().join("src");
    std::fs::create_dir_all(src.join("sub")).expect("dirs");
    let big: Vec<u8> = (0..100_000u32).map(|i| (i % 249) as u8).collect();
    std::fs::write(src.join("a.bin"), &big).expect("a");
    std::fs::write(src.join("sub/b.txt"), "hola").expect("b");
    let remote = format!("/tmp/friki-xfer-scp-{}", std::process::id());

    // Fichero suelto (nombre con comilla para ejercitar el escape).
    let odd = tmp.path().join("it's.txt");
    std::fs::write(&odd, "x").expect("odd");
    let remote_odd = format!("{remote}-it's.txt");
    assert_eq!(
        scp::upload(&conn, &odd, &remote_odd, false)
            .await
            .expect("up"),
        1
    );
    let out = tmp.path().join("out");
    std::fs::create_dir(&out).expect("out");
    assert_eq!(
        scp::download(&conn, &remote_odd, &out, false)
            .await
            .expect("down"),
        1
    );
    // El nombre local es el basename remoto (`friki-xfer-scp-PID-it's.txt`).
    let odd_name = std::path::Path::new(&remote_odd)
        .file_name()
        .expect("nombre");
    assert_eq!(std::fs::read(out.join(odd_name)).expect("leer"), b"x");

    // Recursivo: sube `src` como `remote`, baja de vuelta.
    let n = scp::upload(&conn, &src, &remote, true)
        .await
        .expect("up -r");
    assert_eq!(n, 100_004);
    let back = tmp.path().join("back");
    std::fs::create_dir(&back).expect("back");
    let n = scp::download(&conn, &remote, &back, true)
        .await
        .expect("down -r");
    assert_eq!(n, 100_004);
    let name = std::path::Path::new(&remote).file_name().expect("nombre");
    assert_eq!(
        std::fs::read(back.join(name).join("a.bin")).expect("a"),
        big
    );
    assert_eq!(
        std::fs::read(back.join(name).join("sub/b.txt")).expect("b"),
        b"hola"
    );

    conn.exec(&format!(
        "rm -rf '{remote}' '{}'",
        remote_odd.replace('\'', "'\\''")
    ))
    .await
    .expect("limpieza");
    conn.disconnect().await.expect("disconnect");
}
