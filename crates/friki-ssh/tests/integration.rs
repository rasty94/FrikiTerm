//! Pruebas contra un OpenSSH real (`scripts/test-sshd.sh`). Las que lo necesitan van
//! `#[ignore]`: `cargo test -p friki-ssh -- --include-ignored`.
//!
//! Sin las variables `FRIKI_TEST_*` cada prueba lo dice por stderr y termina; con
//! `FRIKI_TEST_REQUIRED=1` esa falta es un fallo (para CI), nunca un "ok" vacío.

use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use friki_core::config::{AuthMethod, SessionConfig};
use friki_core::known_hosts::{HostKey, HostKeyStatus};
use friki_ssh::{
    AuthPrompter, Connection, Error, HostKeyDecision, HostKeyVerifier, KnownHostsVerifier,
    ShellEvent, Zeroizing,
};
use tokio::io::AsyncReadExt;

struct Env {
    host: String,
    port: u16,
    legacy_port: Option<u16>,
    user: String,
    password: String,
    key: PathBuf,
    known_hosts: PathBuf,
}

fn required() -> bool {
    std::env::var("FRIKI_TEST_REQUIRED").is_ok_and(|v| v == "1")
}

/// Si falta algo: `panic!` con `FRIKI_TEST_REQUIRED=1`; si no, aviso claro y `None`.
fn skip(what: &str) -> Option<()> {
    assert!(
        !required(),
        "FRIKI_TEST_REQUIRED=1 y falta {what}: levanta el servidor con scripts/test-sshd.sh up"
    );
    eprintln!("SALTADA (no ejecutada): falta {what}");
    None
}

fn env() -> Option<Env> {
    let v = |n: &str| std::env::var(n).ok().filter(|s| !s.is_empty());
    let e = (|| {
        Some(Env {
            host: v("FRIKI_TEST_HOST")?,
            port: v("FRIKI_TEST_PORT")?.parse().ok()?,
            legacy_port: v("FRIKI_TEST_LEGACY_PORT").and_then(|p| p.parse().ok()),
            user: v("FRIKI_TEST_USER")?,
            password: v("FRIKI_TEST_PASSWORD")?,
            key: v("FRIKI_TEST_KEY")?.into(),
            known_hosts: v("FRIKI_TEST_KNOWN_HOSTS")?.into(),
        })
    })();
    if e.is_none() {
        skip("alguna variable FRIKI_TEST_*")?;
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

/// Responde con contraseña fija y cuenta cuántas veces se le pidió.
struct Prompter(String, Arc<AtomicUsize>);

fn prompter(pw: &str) -> Arc<Prompter> {
    Arc::new(Prompter(pw.to_string(), Arc::new(AtomicUsize::new(0))))
}

#[async_trait::async_trait]
impl AuthPrompter for Prompter {
    async fn password(&self, _: &str, _: &str) -> Option<Zeroizing<String>> {
        self.1.fetch_add(1, Ordering::SeqCst);
        Some(Zeroizing::new(self.0.clone()))
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

type Ask = Pin<Box<dyn Future<Output = HostKeyDecision> + Send>>;

async fn connect_key(e: &Env) -> Connection {
    Connection::connect(
        &cfg(e, vec![AuthMethod::PublicKey]),
        Arc::new(AcceptAll),
        prompter(""),
    )
    .await
    .expect("conexión con clave")
}

#[tokio::test]
#[ignore = "requiere OpenSSH de pruebas"]
async fn conecta_con_password_y_ejecuta_echo() {
    let Some(e) = env() else { return };
    let c = cfg(&e, vec![AuthMethod::Password]);
    let conn = Connection::connect(&c, Arc::new(AcceptAll), prompter(&e.password))
        .await
        .expect("conexión con contraseña");
    let out = conn.exec("echo hola-friki").await.expect("exec");
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "hola-friki");
    assert_eq!(out.exit_status, Some(0));
    assert_eq!(out.exit_signal, None);
    let n = conn.negotiated().expect("negociado");
    assert!(
        n.kex.starts_with("curve25519") || n.kex.starts_with("ecdh"),
        "{n:?}"
    );
    assert!(conn.warnings().is_empty(), "{:?}", conn.warnings());
    conn.disconnect().await.expect("disconnect");
}

#[tokio::test]
#[ignore = "requiere OpenSSH de pruebas"]
async fn conecta_con_clave() {
    let Some(e) = env() else { return };
    // Contraseña errónea a propósito: solo puede entrar por la clave.
    let conn = connect_key(&e).await;
    let out = conn.exec("echo clave-ok").await.expect("exec");
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "clave-ok");
}

#[tokio::test]
#[ignore = "requiere OpenSSH de pruebas"]
async fn contrasena_mala_se_repite_hasta_agotar_intentos() {
    let Some(e) = env() else { return };
    let c = cfg(&e, vec![AuthMethod::Password]);
    let p = prompter("mal");
    let r = Connection::connect(&c, Arc::new(AcceptAll), p.clone()).await;
    assert!(matches!(r, Err(Error::Auth(_))), "{:?}", r.err());
    assert_eq!(p.1.load(Ordering::SeqCst), c.max_auth_attempts as usize);
}

#[cfg(unix)]
#[tokio::test]
#[ignore = "requiere OpenSSH de pruebas"]
async fn clave_con_permisos_abiertos_se_ignora_con_aviso() {
    let Some(e) = env() else { return };
    {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().expect("tempdir");
        let copia = dir.path().join("id");
        std::fs::copy(&e.key, &copia).expect("copia");
        std::fs::set_permissions(&copia, std::fs::Permissions::from_mode(0o644)).expect("chmod");
        let mut c = cfg(&e, vec![AuthMethod::PublicKey]);
        c.identity_files = vec![copia];
        let r = Connection::connect(&c, Arc::new(AcceptAll), prompter("")).await;
        assert!(matches!(r, Err(Error::Auth(_))), "{:?}", r.err());
    }
}

#[tokio::test]
#[ignore = "requiere OpenSSH de pruebas"]
async fn abre_shell_y_ejecuta() {
    let Some(e) = env() else { return };
    let conn = connect_key(&e).await;
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
                other => panic!("evento inesperado: {other:?}"),
            }
        }
    };
    tokio::time::timeout(Duration::from_secs(15), leer)
        .await
        .expect("el shell no terminó");
    assert!(String::from_utf8_lossy(&texto).contains("SHELL-42"));
    assert_eq!(salida, Some(3));
    assert!(cerrado);
}

/// Regresión del bloqueo: salida masiva sin leer no puede impedir escribir (Ctrl-C),
/// ni congelar la conexión.
#[tokio::test]
#[ignore = "requiere OpenSSH de pruebas"]
async fn escribir_sin_leer_con_salida_masiva_no_se_bloquea() {
    let Some(e) = env() else { return };
    let conn = connect_key(&e).await;
    let (w, mut r) = conn.open_shell("xterm", 80, 24).await.expect("shell");
    w.write(b"yes\n").await.expect("write yes");
    tokio::time::sleep(Duration::from_secs(2)).await; // sin leer
    let t = std::time::Instant::now();
    tokio::time::timeout(Duration::from_secs(5), async {
        w.write(&[0x03]).await.expect("ctrl-c");
        w.resize(120, 40).await.expect("resize");
        w.write(b"exit\n").await.expect("exit");
    })
    .await
    .expect("la escritura se bloqueó");
    assert!(t.elapsed() < Duration::from_secs(5));
    // La conexión sigue viva aunque la salida no se haya leído.
    let out = tokio::time::timeout(Duration::from_secs(5), conn.exec("echo vivo"))
        .await
        .expect("la conexión quedó bloqueada")
        .expect("exec");
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "vivo");
    // Al leer, el shell termina (limpio, o `Lost` si la salida desbordó el búfer).
    let fin = tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            match r.next_event().await {
                Some(ShellEvent::Closed | ShellEvent::Lost(_)) | None => break,
                Some(_) => {}
            }
        }
    })
    .await;
    assert!(fin.is_ok(), "el shell no terminó");
}

/// `kill -9 $PPID` en el shell remoto mata la sesión sin estado de salida:
/// debe llegar `Lost`, nunca `Closed`.
#[tokio::test]
#[ignore = "requiere OpenSSH de pruebas"]
async fn matar_el_shell_remoto_da_lost() {
    let Some(e) = env() else { return };
    let conn = connect_key(&e).await;
    let (w, mut r) = conn.open_shell("xterm", 80, 24).await.expect("shell");
    w.write(b"kill -9 $PPID\n").await.expect("write");
    let ultimo = tokio::time::timeout(Duration::from_secs(15), async {
        let mut ultimo = None;
        while let Some(ev) = r.next_event().await {
            if !matches!(ev, ShellEvent::Data(_)) {
                ultimo = Some(ev);
            }
        }
        ultimo
    })
    .await
    .expect("no terminó");
    assert!(matches!(ultimo, Some(ShellEvent::Lost(_))), "{ultimo:?}");
}

#[tokio::test]
#[ignore = "requiere OpenSSH de pruebas"]
async fn exec_envia_eof_y_no_se_cuelga() {
    let Some(e) = env() else { return };
    let conn = connect_key(&e).await;
    let out = tokio::time::timeout(Duration::from_secs(10), conn.exec("cat"))
        .await
        .expect("exec colgado sin EOF")
        .expect("exec");
    assert_eq!(out.exit_status, Some(0));
    assert!(out.stdout.is_empty());
}

#[tokio::test]
#[ignore = "requiere OpenSSH de pruebas"]
async fn exec_streaming_con_stdin() {
    let Some(e) = env() else { return };
    let conn = connect_key(&e).await;
    let (mut o, mut er) = (Vec::new(), Vec::new());
    let st = conn
        .exec_streaming("cat; echo fin >&2", &b"hola"[..], &mut o, &mut er)
        .await
        .expect("exec_streaming");
    assert_eq!(st.exit_status, Some(0));
    assert_eq!(o, b"hola");
    assert_eq!(String::from_utf8_lossy(&er).trim(), "fin");
}

#[tokio::test]
#[ignore = "requiere OpenSSH de pruebas"]
async fn exec_muerto_por_senal_da_exit_signal() {
    let Some(e) = env() else { return };
    let conn = connect_key(&e).await;
    let out = conn.exec("kill -9 $$").await.expect("exec");
    assert_eq!(out.exit_status, None, "{:?}", out.exit_signal);
    assert_eq!(out.exit_signal.as_deref(), Some("KILL"));
}

#[tokio::test]
#[ignore = "requiere OpenSSH de pruebas"]
async fn exec_con_conexion_cortada_da_error_de_red() {
    let Some(e) = env() else { return };
    let conn = connect_key(&e).await;
    // Mata la sesión sshd que ejecuta la orden: el canal se cierra sin estado.
    let r = conn.exec("kill -9 $PPID").await;
    assert!(
        matches!(r, Err(Error::Network(_))),
        "{:?}",
        r.map(|o| o.exit_status)
    );
}

/// El plazo de conexión no cuenta el tiempo del diálogo de clave de host.
#[tokio::test]
#[ignore = "requiere OpenSSH de pruebas"]
async fn el_dialogo_de_clave_no_consume_el_timeout() {
    let Some(e) = env() else { return };
    struct Lento;
    #[async_trait::async_trait]
    impl HostKeyVerifier for Lento {
        async fn verify(&self, _: &str, _: u16, _: &HostKey) -> HostKeyDecision {
            tokio::time::sleep(Duration::from_millis(2500)).await;
            HostKeyDecision::AcceptOnce
        }
    }
    let mut c = cfg(&e, vec![AuthMethod::PublicKey]);
    c.connect_timeout_secs = 1;
    let conn = Connection::connect(&c, Arc::new(Lento), prompter(""))
        .await
        .expect("el diálogo lento no debe agotar el plazo");
    conn.disconnect().await.expect("disconnect");
}

// Con la feature `legacy` el aviso esperado es otro (algoritmos heredados habilitados).
#[cfg(not(feature = "legacy"))]
#[tokio::test]
#[ignore = "requiere OpenSSH de pruebas"]
async fn legacy_sin_la_feature_avisa() {
    let Some(e) = env() else { return };
    let mut c = cfg(&e, vec![AuthMethod::PublicKey]);
    c.legacy_algorithms = true;
    let conn = Connection::connect(&c, Arc::new(AcceptAll), prompter(""))
        .await
        .expect("conexión");
    assert!(
        conn.warnings().iter().any(|w| w.contains("legacy")),
        "{:?}",
        conn.warnings()
    );
}

fn known_hosts_con_otra_clave(e: &Env, dir: &Path) -> PathBuf {
    let otra = friki_ssh::keys::generate_ed25519(&dir.join("otra"), "otra", None).expect("keygen");
    let b64 = otra.split_whitespace().nth(1).expect("base64").to_string();
    let original = std::fs::read_to_string(&e.known_hosts).expect("known_hosts");
    let falsificado: Vec<String> = original
        .lines()
        .filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
        .filter_map(|l| {
            let mut p: Vec<&str> = l.split_whitespace().collect();
            (p.len() >= 3 && p[1] == "ssh-ed25519").then(|| {
                p[2] = &b64;
                p.join(" ")
            })
        })
        .collect();
    assert!(!falsificado.is_empty(), "known_hosts sin ssh-ed25519");
    let path = dir.join("known_hosts");
    std::fs::write(&path, falsificado.join("\n") + "\n").expect("escribir");
    path
}

#[tokio::test]
#[ignore = "requiere OpenSSH de pruebas"]
async fn clave_de_host_cambiada_falla() {
    let Some(e) = env() else { return };
    let dir = tempfile::tempdir().expect("tempdir");
    let path = known_hosts_con_otra_clave(&e, dir.path());
    let v = KnownHostsVerifier::new(
        path.clone(),
        |_: &str, _: u16, _: &HostKey, s: HostKeyStatus| {
            assert!(matches!(s, HostKeyStatus::Changed { .. }), "{s:?}");
            Box::pin(async { HostKeyDecision::Reject }) as Ask
        },
    );
    let before = std::fs::read_to_string(&path).expect("leer");
    let r = Connection::connect(
        &cfg(&e, vec![AuthMethod::PublicKey]),
        Arc::new(v),
        prompter(""),
    )
    .await;
    assert!(matches!(r, Err(Error::HostKey(_))), "{:?}", r.err());
    assert_eq!(std::fs::read_to_string(&path).expect("leer"), before);
}

#[tokio::test]
#[ignore = "requiere OpenSSH de pruebas"]
async fn clave_de_host_conocida_no_pregunta() {
    let Some(e) = env() else { return };
    let v = KnownHostsVerifier::new(
        e.known_hosts.clone(),
        |_: &str, _: u16, _: &HostKey, _: HostKeyStatus| {
            Box::pin(async { HostKeyDecision::Reject }) as Ask
        },
    );
    let conn = Connection::connect(
        &cfg(&e, vec![AuthMethod::PublicKey]),
        Arc::new(v),
        prompter(""),
    )
    .await
    .expect("clave conocida");
    assert!(conn.warnings().is_empty(), "{:?}", conn.warnings());
}

#[tokio::test]
#[ignore = "requiere OpenSSH de pruebas"]
async fn servidor_heredado_sin_algoritmos_comunes() {
    let Some(e) = env() else { return };
    let Some(port) = e.legacy_port else {
        skip("FRIKI_TEST_LEGACY_PORT");
        return;
    };
    let mut c = cfg(&e, vec![AuthMethod::Password]);
    c.port = port;
    let r = Connection::connect(&c, Arc::new(AcceptAll), prompter(&e.password)).await;
    assert!(matches!(r, Err(Error::Negotiation(_))), "{:?}", r.err());
}

/// No necesita servidor: un falso lee el KEXINIT del cliente y comprueba que
/// anuncia strict KEX (CVE-2023-48795) y ninguna compresión.
#[tokio::test]
async fn el_cliente_anuncia_strict_kex_y_sin_compresion() {
    use tokio::io::AsyncWriteExt;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let port = listener.local_addr().expect("addr").port();
    let servidor = tokio::spawn(async move {
        let (mut s, _) = listener.accept().await.expect("accept");
        s.write_all(b"SSH-2.0-falso\r\n").await.expect("version");
        let mut recibido = Vec::new();
        let mut buf = [0u8; 4096];
        let _ = tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                match s.read(&mut buf).await {
                    Ok(0) | Err(_) => break,
                    Ok(n) => recibido.extend_from_slice(&buf[..n]),
                }
                if recibido.windows(5).any(|w| w == b"zlib@") || recibido.len() > 1500 {
                    break;
                }
            }
        })
        .await;
        recibido
    });
    let c = SessionConfig {
        host: "127.0.0.1".into(),
        port,
        user: Some("x".into()),
        connect_timeout_secs: 2,
        ..Default::default()
    };
    let _ = Connection::connect(&c, Arc::new(AcceptAll), prompter("")).await;
    let recibido = servidor.await.expect("servidor");
    let texto = String::from_utf8_lossy(&recibido);
    assert!(texto.contains("kex-strict-c-v00@openssh.com"), "{texto}");
    assert!(texto.contains("ext-info-c"));
    assert!(!texto.contains("zlib"));
}

// ---------- Fases 3 y 4: flujos, ProxyJump, redirecciones ----------

use friki_core::config::Forward;
use friki_core::transport::StreamEvent;
use tokio::io::AsyncWriteExt;

/// Lee hasta fin de línea (o 5 s) y devuelve lo recibido.
async fn read_line_of<R: tokio::io::AsyncRead + Unpin>(r: &mut R) -> String {
    let mut got = Vec::new();
    let mut b = [0u8; 1];
    let read = async {
        while let Ok(1) = r.read(&mut b).await {
            got.push(b[0]);
            if b[0] == b'\n' {
                break;
            }
        }
    };
    tokio::time::timeout(Duration::from_secs(5), read)
        .await
        .expect("plazo leyendo la línea");
    String::from_utf8_lossy(&got).into_owned()
}

#[tokio::test]
#[ignore = "requiere OpenSSH de pruebas"]
async fn shell_stream_entrega_datos_y_codigo_de_salida() {
    let Some(e) = env() else { return };
    let conn = connect_key(&e).await;
    let (w, mut r) = conn
        .open_shell_stream("xterm", 80, 24)
        .await
        .expect("shell");
    w.resize(100, 30).await.expect("resize");
    w.write(b"echo hola-stream; exit 7\n").await.expect("write");
    let (mut text, mut exit, mut closed) = (Vec::new(), None, false);
    let run = async {
        while let Some(ev) = r.next_event().await {
            match ev {
                StreamEvent::Data(d) => text.extend(d),
                StreamEvent::Exit(n) => exit = Some(n),
                StreamEvent::Closed => closed = true,
                StreamEvent::Lost(m) => panic!("perdido: {m}"),
            }
        }
    };
    tokio::time::timeout(Duration::from_secs(10), run)
        .await
        .expect("plazo");
    assert!(String::from_utf8_lossy(&text).contains("hola-stream"));
    assert_eq!(exit, Some(7));
    assert!(closed, "Closed llega tras el estado de salida");
}

#[tokio::test]
#[ignore = "requiere OpenSSH de pruebas"]
async fn subsistema_sftp_responde_version() {
    let Some(e) = env() else { return };
    let conn = connect_key(&e).await;
    let mut s = conn.open_subsystem("sftp").await.expect("sftp");
    // SSH_FXP_INIT (1) con versión 3.
    s.write_all(&[0, 0, 0, 5, 1, 0, 0, 0, 3])
        .await
        .expect("init");
    let mut head = [0u8; 5];
    tokio::time::timeout(Duration::from_secs(5), s.read_exact(&mut head))
        .await
        .expect("plazo")
        .expect("cabecera");
    assert_eq!(head[4], 2, "SSH_FXP_VERSION: {head:?}");
}

#[tokio::test]
#[ignore = "requiere OpenSSH de pruebas"]
async fn exec_stream_hace_eco_y_da_estado() {
    let Some(e) = env() else { return };
    let conn = connect_key(&e).await;
    let mut s = conn.open_exec_stream("cat").await.expect("cat");
    s.write_all(b"eco-exec\n").await.expect("write");
    s.shutdown().await.expect("eof");
    let mut out = Vec::new();
    tokio::time::timeout(Duration::from_secs(5), s.read_to_end(&mut out))
        .await
        .expect("plazo")
        .expect("leer");
    assert_eq!(out, b"eco-exec\n");
    assert_eq!(s.exit_status(), Some(0));
}

/// Variables del destino de salto, o `None` (con el aviso de siempre).
fn jump_target() -> Option<(String, u16)> {
    let Some(t) = std::env::var("FRIKI_TEST_JUMP_TARGET")
        .ok()
        .filter(|s| !s.is_empty())
    else {
        skip("FRIKI_TEST_JUMP_TARGET");
        return None;
    };
    let (h, p) = t.rsplit_once(':')?;
    Some((h.to_string(), p.parse().ok()?))
}

/// Acepta todo y recuerda para qué `host:puerto` se le preguntó.
#[derive(Default)]
struct Recording(std::sync::Mutex<Vec<(String, u16)>>);

#[async_trait::async_trait]
impl HostKeyVerifier for Recording {
    async fn verify(&self, host: &str, port: u16, _: &HostKey) -> HostKeyDecision {
        self.0.lock().expect("lock").push((host.to_string(), port));
        HostKeyDecision::AcceptOnce
    }
}

#[tokio::test]
#[ignore = "requiere OpenSSH de pruebas"]
async fn proxy_jump_conecta_al_destino_y_verifica_ambas_claves() {
    let Some(e) = env() else { return };
    let Some((thost, tport)) = jump_target() else {
        return;
    };
    let mut c = cfg(&e, vec![AuthMethod::PublicKey]);
    c.host = thost.clone();
    c.port = tport;
    c.proxy_jump = Some(format!("{}@{}:{}", e.user, e.host, e.port));
    let v = Arc::new(Recording::default());
    let conn = Connection::connect(&c, v.clone(), prompter(""))
        .await
        .expect("conexión por el salto");
    let out = conn.exec("echo por-el-salto").await.expect("exec");
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "por-el-salto");
    let asked = v.0.lock().expect("lock").clone();
    assert_eq!(
        asked,
        [(e.host.clone(), e.port), (thost, tport)],
        "una verificación por salto, la del destino con el nombre del destino"
    );
}

#[tokio::test]
#[ignore = "requiere OpenSSH de pruebas"]
async fn proxy_jump_rechaza_la_clave_del_destino() {
    let Some(e) = env() else { return };
    let Some((thost, tport)) = jump_target() else {
        return;
    };
    struct OnlyJump(u16);
    #[async_trait::async_trait]
    impl HostKeyVerifier for OnlyJump {
        async fn verify(&self, _: &str, port: u16, _: &HostKey) -> HostKeyDecision {
            if port == self.0 {
                HostKeyDecision::AcceptOnce
            } else {
                HostKeyDecision::Reject
            }
        }
    }
    let mut c = cfg(&e, vec![AuthMethod::PublicKey]);
    c.host = thost;
    c.port = tport;
    c.proxy_jump = Some(format!("{}:{}", e.host, e.port));
    let r = Connection::connect(&c, Arc::new(OnlyJump(e.port)), prompter("")).await;
    assert!(matches!(r, Err(Error::HostKey(_))), "{:?}", r.err());
}

#[tokio::test]
#[ignore = "requiere OpenSSH de pruebas"]
async fn proxy_jump_mal_formado_no_conecta() {
    let Some(e) = env() else { return };
    let mut c = cfg(&e, vec![AuthMethod::PublicKey]);
    c.proxy_jump = Some("host:xx".into());
    let r = Connection::connect(&c, Arc::new(AcceptAll), prompter("")).await;
    assert!(
        matches!(r, Err(Error::Other(ref m)) if m.contains("ProxyJump")),
        "{:?}",
        r.err()
    );
}

#[tokio::test]
#[ignore = "requiere OpenSSH de pruebas"]
async fn redireccion_local_llega_al_sshd_remoto() {
    let Some(e) = env() else { return };
    let conn = connect_key(&e).await;
    let h = conn
        .start_forward(&Forward::Local {
            bind: String::new(),
            port: 0,
            host: "127.0.0.1".into(),
            host_port: e.port,
        })
        .await
        .expect("forward");
    assert!(h.listen.ip().is_loopback(), "{h:?}");
    assert!(conn.warnings().is_empty(), "{:?}", conn.warnings());
    let mut s = tokio::net::TcpStream::connect(h.listen).await.expect("tcp");
    assert!(read_line_of(&mut s).await.starts_with("SSH-2.0"));
    conn.stop_forward(h.clone()).await.expect("stop");
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(tokio::net::TcpStream::connect(h.listen).await.is_err());
}

#[tokio::test]
#[ignore = "requiere OpenSSH de pruebas"]
async fn redireccion_expuesta_avisa() {
    let Some(e) = env() else { return };
    let conn = connect_key(&e).await;
    let h = conn
        .start_forward(&Forward::Local {
            bind: "*".into(),
            port: 0,
            host: "127.0.0.1".into(),
            host_port: e.port,
        })
        .await
        .expect("forward");
    assert!(
        conn.warnings()
            .iter()
            .any(|w| w.contains("accesible desde la red")),
        "{:?}",
        conn.warnings()
    );
    conn.stop_forward(h).await.expect("stop");
}

#[tokio::test]
#[ignore = "requiere OpenSSH de pruebas"]
async fn redireccion_remota_vuelve_al_destino_local() {
    let Some(e) = env() else { return };
    // Destino local: responde con una línea fija.
    let local = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let local_port = local.local_addr().expect("addr").port();
    tokio::spawn(async move {
        while let Ok((mut s, _)) = local.accept().await {
            let _ = s.write_all(b"hola-remoto\n").await;
        }
    });

    let a = connect_key(&e).await;
    let r = a
        .start_forward(&Forward::Remote {
            bind: String::new(),
            port: 0,
            host: "127.0.0.1".into(),
            host_port: local_port,
        })
        .await
        .expect("remote");
    assert_ne!(r.listen.port(), 0);

    // Segunda conexión: llega al puerto que `a` abrió en el servidor.
    let b = connect_key(&e).await;
    let l = b
        .start_forward(&Forward::Local {
            bind: String::new(),
            port: 0,
            host: "127.0.0.1".into(),
            host_port: r.listen.port(),
        })
        .await
        .expect("local");
    let mut s = tokio::net::TcpStream::connect(l.listen).await.expect("tcp");
    assert_eq!(read_line_of(&mut s).await, "hola-remoto\n");
    a.stop_forward(r).await.expect("cancelar");
}

async fn socks_connect(
    listen: std::net::SocketAddr,
    atyp_and_addr: &[u8],
    port: u16,
) -> tokio::net::TcpStream {
    let mut s = tokio::net::TcpStream::connect(listen).await.expect("tcp");
    s.write_all(&[5, 1, 0]).await.expect("saludo");
    let mut m = [0u8; 2];
    s.read_exact(&mut m).await.expect("método");
    assert_eq!(m, [5, 0]);
    let mut req = vec![5, 1, 0];
    req.extend(atyp_and_addr);
    req.extend(port.to_be_bytes());
    s.write_all(&req).await.expect("petición");
    let mut rep = [0u8; 10];
    s.read_exact(&mut rep).await.expect("respuesta");
    assert_eq!(rep[1], 0, "éxito: {rep:?}");
    s
}

#[tokio::test]
#[ignore = "requiere OpenSSH de pruebas"]
async fn redireccion_dinamica_socks5() {
    let Some(e) = env() else { return };
    let conn = connect_key(&e).await;
    let h = conn
        .start_forward(&Forward::Dynamic {
            bind: String::new(),
            port: 0,
        })
        .await
        .expect("socks");

    // IPv4
    let mut s = socks_connect(h.listen, &[1, 127, 0, 0, 1], e.port).await;
    assert!(read_line_of(&mut s).await.starts_with("SSH-2.0"));
    // Dominio
    let mut name = vec![3, 9];
    name.extend(b"localhost");
    let mut s = socks_connect(h.listen, &name, e.port).await;
    assert!(read_line_of(&mut s).await.starts_with("SSH-2.0"));
    // IPv6 loopback (si el contenedor lo tiene el banner llega; si no, se rechaza con error SOCKS)
    let mut v6 = vec![4];
    v6.extend(std::net::Ipv6Addr::LOCALHOST.octets());
    let mut s = tokio::net::TcpStream::connect(h.listen).await.expect("tcp");
    s.write_all(&[5, 1, 0]).await.expect("saludo");
    let mut m = [0u8; 2];
    s.read_exact(&mut m).await.expect("método");
    let mut req = vec![5, 1, 0];
    req.extend(&v6);
    req.extend(e.port.to_be_bytes());
    s.write_all(&req).await.expect("petición");
    let mut rep = [0u8; 10];
    s.read_exact(&mut rep).await.expect("respuesta IPv6");
    assert_eq!(rep[0], 5);

    // BIND no se admite.
    let mut s = tokio::net::TcpStream::connect(h.listen).await.expect("tcp");
    // Solo la cabecera: el servidor cierra tras rechazar y no debe quedar nada sin leer (RST).
    s.write_all(&[5, 1, 0, 5, 2, 0, 1]).await.expect("bind");
    let mut buf = Vec::new();
    s.read_to_end(&mut buf).await.expect("leer");
    assert_eq!(buf[..2], [5, 0]);
    assert_eq!(buf[2..4], [5, 7]);
}

#[tokio::test]
#[ignore = "requiere OpenSSH de pruebas"]
async fn require_strict_kex_conecta_con_openssh() {
    let Some(e) = env() else { return };
    let mut c = cfg(&e, vec![AuthMethod::PublicKey]);
    c.require_strict_kex = true;
    let conn = Connection::connect(&c, Arc::new(AcceptAll), prompter(""))
        .await
        .expect("OpenSSH anuncia strict KEX");
    conn.exec("true").await.expect("exec");
}
