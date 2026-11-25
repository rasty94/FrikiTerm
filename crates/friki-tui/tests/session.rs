//! Multiplexor de la TUI contra un OpenSSH real, sin terminal: `TestBackend` y teclas
//! sintéticas. `scripts/test-sshd.sh up` exporta las variables `FRIKI_TEST_*`.
//! Sin ellas cada prueba lo dice por stderr y termina; con `FRIKI_TEST_REQUIRED=1`
//! esa falta es un fallo (para CI).
//!
//! `cargo test -p friki-tui -- --include-ignored`

use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use friki_core::config::{AuthMethod, SessionConfig};
use friki_core::known_hosts::{HostKey, HostKeyStatus};
use friki_ssh::{AuthPrompter, Connection, HostKeyDecision, KnownHostsVerifier, Zeroizing};
use friki_tui::{Paths, run_with};
use ratatui::Terminal;
use ratatui::backend::{Backend, ClearType, TestBackend, WindowSize};
use ratatui::buffer::Cell;
use ratatui::layout::{Position, Size};
use tokio::sync::mpsc;

const W: u16 = 100;
const H: u16 = 25;

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

/// `TestBackend` compartido para poder leer la pantalla mientras la TUI corre.
#[derive(Clone)]
struct Shared(Arc<Mutex<TestBackend>>);

impl Shared {
    fn new() -> Self {
        Self(Arc::new(Mutex::new(TestBackend::new(W, H))))
    }

    fn lock(&self) -> MutexGuard<'_, TestBackend> {
        self.0.lock().expect("backend")
    }

    /// Filas de la pantalla como vectores de símbolos.
    fn rows(&self) -> Vec<Vec<String>> {
        let b = self.lock();
        b.buffer()
            .content()
            .chunks(usize::from(W))
            .map(|r| r.iter().map(|c| c.symbol().to_string()).collect())
            .collect()
    }

    fn text(&self) -> String {
        self.rows().iter().map(|r| r.concat() + "\n").collect()
    }

    /// Texto de las columnas `[from, to)`.
    fn region(&self, from: usize, to: usize) -> String {
        self.rows()
            .iter()
            .map(|r| r[from..to].concat() + "\n")
            .collect()
    }
}

impl Backend for Shared {
    type Error = std::convert::Infallible;

    fn draw<'a, I>(&mut self, content: I) -> Result<(), Self::Error>
    where
        I: Iterator<Item = (u16, u16, &'a Cell)>,
    {
        self.lock().draw(content)
    }
    fn hide_cursor(&mut self) -> Result<(), Self::Error> {
        self.lock().hide_cursor()
    }
    fn show_cursor(&mut self) -> Result<(), Self::Error> {
        self.lock().show_cursor()
    }
    fn get_cursor_position(&mut self) -> Result<Position, Self::Error> {
        self.lock().get_cursor_position()
    }
    fn set_cursor_position<P: Into<Position>>(&mut self, position: P) -> Result<(), Self::Error> {
        self.lock().set_cursor_position(position)
    }
    fn clear(&mut self) -> Result<(), Self::Error> {
        self.lock().clear()
    }
    fn clear_region(&mut self, clear_type: ClearType) -> Result<(), Self::Error> {
        self.lock().clear_region(clear_type)
    }
    fn size(&self) -> Result<Size, Self::Error> {
        self.lock().size()
    }
    fn window_size(&mut self) -> Result<WindowSize, Self::Error> {
        self.lock().window_size()
    }
    fn flush(&mut self) -> Result<(), Self::Error> {
        self.lock().flush()
    }
}

fn key(c: KeyCode) -> Event {
    Event::Key(KeyEvent::new(c, KeyModifiers::NONE))
}

struct Keys(mpsc::UnboundedSender<std::io::Result<Event>>);

impl Keys {
    fn send(&self, ev: Event) {
        self.0.send(Ok(ev)).expect("evento");
    }

    fn typed(&self, s: &str) {
        for c in s.chars() {
            self.send(key(if c == '\n' {
                KeyCode::Enter
            } else {
                KeyCode::Char(c)
            }));
        }
    }

    /// Prefijo `Ctrl+]` y la tecla de la acción.
    fn prefixed(&self, c: char) {
        self.send(Event::Key(KeyEvent::new(
            KeyCode::Char(']'),
            KeyModifiers::CONTROL,
        )));
        self.send(key(KeyCode::Char(c)));
    }
}

async fn wait_for(what: &str, mut ok: impl FnMut() -> bool) {
    let t0 = Instant::now();
    while !ok() {
        assert!(
            t0.elapsed() < Duration::from_secs(20),
            "tiempo agotado: {what}"
        );
        tokio::time::sleep(Duration::from_millis(40)).await;
    }
}

/// Ejecuta la TUI con `initial` y un guion concurrente. `files` son ficheros de sesión
/// (`nombre.toml`, contenido) del selector. Devuelve el resultado y la pantalla final.
async fn run_tui<F, Fut>(
    initial: SessionConfig,
    known_hosts: PathBuf,
    tag: &str,
    files: &[(String, String)],
    exit_with_last: bool,
    script: F,
) -> (anyhow::Result<i32>, Shared)
where
    F: FnOnce(Keys, Shared) -> Fut,
    Fut: Future<Output = ()>,
{
    let dir = std::env::temp_dir().join(format!("friki-tui-test-{}-{tag}", std::process::id()));
    let sessions_dir = dir.join("sessions");
    std::fs::create_dir_all(&sessions_dir).expect("dir de sesiones");
    for (name, toml) in files {
        std::fs::write(sessions_dir.join(name), toml).expect("sesión");
    }
    let paths = Paths {
        known_hosts,
        logs_dir: dir.join("logs"),
        sessions_dir,
        downloads_dir: dir.join("downloads"),
        scripts_dir: dir.join("scripts"),
    };
    let shared = Shared::new();
    let mut tui = Terminal::new(shared.clone()).expect("terminal");
    let (tx, rx) = mpsc::unbounded_channel::<std::io::Result<Event>>();
    let stream = Box::pin(futures_util::stream::unfold(rx, |mut rx| async move {
        rx.recv().await.map(|ev| (ev, rx))
    }));
    let run = run_with(&mut tui, stream, Some(initial), &paths, exit_with_last);
    let keep = tx.clone(); // el flujo no se cierra cuando acaba el guion
    let guion = script(Keys(tx), shared.clone());
    let (res, ()) =
        tokio::time::timeout(Duration::from_secs(90), async { tokio::join!(run, guion) })
            .await
            .expect("la prueba se colgó");
    drop(keep);
    (res, shared)
}

/// Sesión SSH inicial más las `sessions` guardadas en el selector.
async fn drive<F, Fut>(
    e: &Env,
    sessions: &[SessionConfig],
    exit_with_last: bool,
    script: F,
) -> (anyhow::Result<i32>, Shared)
where
    F: FnOnce(Keys, Shared) -> Fut,
    Fut: Future<Output = ()>,
{
    drive_with(e, sessions, &[], exit_with_last, script).await
}

async fn drive_with<F, Fut>(
    e: &Env,
    sessions: &[SessionConfig],
    extra: &[(String, String)],
    exit_with_last: bool,
    script: F,
) -> (anyhow::Result<i32>, Shared)
where
    F: FnOnce(Keys, Shared) -> Fut,
    Fut: Future<Output = ()>,
{
    let initial = session(e);
    trust_host(e, &initial).await;
    let mut files: Vec<(String, String)> = sessions
        .iter()
        .map(|s| {
            let toml = format!(
                "name = {:?}\nhost = {:?}\nport = {}\nuser = {:?}\nauth = [\"public-key\"]\nidentity_files = [{:?}]\nconnect_timeout_secs = 10\nterm = \"xterm-256color\"\n",
                s.name,
                s.host,
                s.port,
                s.user.clone().unwrap_or_default(),
                s.identity_files[0].display().to_string(),
            );
            (format!("{}.toml", s.name), toml)
        })
        .collect();
    files.extend_from_slice(extra);
    let tag = format!("{}-{}", sessions.len(), extra.len());
    run_tui(
        initial,
        e.known_hosts.clone(),
        &tag,
        &files,
        exit_with_last,
        script,
    )
    .await
}

#[tokio::test]
#[ignore = "requiere OpenSSH de pruebas"]
async fn una_sesion_ejecuta_y_devuelve_el_codigo_de_salida() {
    let Some(e) = env() else { return };
    let (res, screen) = drive(&e, &[], true, |keys, sc| async move {
        wait_for("conectado", || sc.text().contains("conectado")).await;
        keys.typed("echo friki-$((40+2))\n");
        wait_for("salida", || sc.text().contains("friki-42")).await;
        keys.typed("exit 7\n");
    })
    .await;
    // Un solo panel: al terminar el remoto la aplicación sale sola, con su código.
    assert_eq!(res.expect("sesión"), 7);
    let text = screen.text();
    assert!(
        text.contains("friki-42") && text.contains("prueba"),
        "{text}"
    );
}

#[tokio::test]
#[ignore = "requiere OpenSSH de pruebas"]
async fn prefijo_q_cierra_la_sesion() {
    let Some(e) = env() else { return };
    let (res, _) = drive(&e, &[], true, |keys, sc| async move {
        wait_for("conectado", || sc.text().contains("conectado")).await;
        keys.typed("echo listo\n");
        wait_for("salida", || sc.text().contains("listo")).await;
        keys.prefixed('q');
    })
    .await;
    assert_eq!(res.expect("sesión"), 0);
}

#[tokio::test]
#[ignore = "requiere OpenSSH de pruebas"]
async fn dos_paneles_a_la_misma_maquina_son_independientes() {
    let Some(e) = env() else { return };
    let saved = [session(&e)];
    let (res, screen) = drive(&e, &saved, false, |keys, sc| async move {
        wait_for("primer panel", || sc.text().contains("conectado")).await;
        // Dividir (izquierda/derecha) y abrir la sesión guardada en el panel nuevo.
        keys.prefixed('%');
        wait_for("selector", || sc.text().contains("Abrir sesión")).await;
        keys.typed("prueba\n");
        tokio::time::sleep(Duration::from_millis(300)).await;
        wait_for("segundo panel", || {
            let t = sc.text();
            t.contains("conectado") && !t.contains("conectando")
        })
        .await;
        // Mitad izquierda: columnas 0..50; derecha: 51..100 (el separador es la 50).
        let (left, right) = (sc.region(0, 50), sc.region(51, 100));
        assert!(
            left.contains("prueba") && right.contains("prueba"),
            "{left}{right}"
        );

        // Foco en el panel nuevo (derecha).
        keys.typed("echo AAA$((1+1))\n");
        wait_for("AAA en la derecha", || sc.region(51, 100).contains("AAA2")).await;
        keys.prefixed('o'); // al panel de la izquierda
        keys.typed("echo BBB$((2+2))\n");
        wait_for("BBB en la izquierda", || sc.region(0, 50).contains("BBB4")).await;
        assert!(
            !sc.region(0, 50).contains("AAA2"),
            "AAA llegó a la izquierda"
        );
        assert!(
            !sc.region(51, 100).contains("BBB4"),
            "BBB llegó a la derecha"
        );

        // Cerrar el panel con foco (izquierda): sigue el otro, a pantalla completa.
        keys.prefixed('x');
        wait_for("panel único", || !sc.text().contains("BBB4")).await;
        keys.typed("echo CCC$((3+3))\n");
        wait_for("CCC", || sc.text().contains("CCC6")).await;
        assert!(
            sc.text().contains("AAA2"),
            "se perdió el historial del otro panel"
        );
        keys.typed("exit 3\n");
        wait_for("terminada", || {
            sc.text().contains("sesión terminada: código 3")
        })
        .await;
        keys.send(key(KeyCode::Char('x'))); // cualquier tecla cierra el panel
    })
    .await;
    // Dos paneles abiertos en total: al cerrar el último, 0.
    assert_eq!(res.expect("aplicación"), 0);
    drop(screen);
}

fn telnet_port() -> Option<u16> {
    let p = std::env::var("FRIKI_TEST_TELNET_PORT")
        .ok()
        .and_then(|v| v.parse().ok());
    if p.is_none() {
        assert!(
            !required(),
            "FRIKI_TEST_REQUIRED=1 y falta FRIKI_TEST_TELNET_PORT: scripts/test-telnetd.sh up"
        );
        eprintln!("SALTADA (no ejecutada): falta FRIKI_TEST_TELNET_PORT");
    }
    p
}

#[tokio::test]
#[ignore = "requiere OpenSSH y telnetd de pruebas"]
async fn un_panel_ssh_y_otro_telnet_a_la_vez() {
    let (Some(e), Some(port)) = (env(), telnet_port()) else {
        return;
    };
    let telnet = (
        "tel.toml".to_string(),
        format!(
            "name = \"tel\"\nprotocol = \"telnet\"\nhost = \"127.0.0.1\"\nport = {port}\nconnect_timeout_secs = 10\n"
        ),
    );
    let (res, screen) = drive_with(&e, &[], &[telnet], false, |keys, sc| async move {
        wait_for("ssh conectado", || sc.text().contains("conectado")).await;
        keys.typed("echo SSH$((20+22))\n");
        wait_for("salida ssh", || sc.text().contains("SSH42")).await;
        // Panel nuevo a la derecha con la sesión Telnet.
        keys.prefixed('%');
        wait_for("selector", || sc.text().contains("Abrir sesión")).await;
        keys.typed("tel\n");
        wait_for("telnet en el selector cerrado", || {
            let t = sc.text();
            !t.contains("Abrir sesión") && t.contains("telnet 127.0.0.1")
        })
        .await;
        // El servidor Telnet muestra algo (banner o login) en el panel derecho.
        wait_for("datos telnet", || {
            sc.region(51, 100)
                .chars()
                .filter(|c| c.is_alphanumeric())
                .count()
                > 8
        })
        .await;
        assert!(sc.region(0, 50).contains("SSH42"), "se perdió el panel SSH");
        assert!(!sc.region(51, 100).contains("SSH42"));
        keys.prefixed('q');
    })
    .await;
    assert_eq!(res.expect("aplicación"), 0);
    drop(screen);
}

#[tokio::test]
#[ignore = "requiere /bin/sh y un PTY local"]
async fn panel_local_shell_termina_con_su_codigo() {
    let initial = SessionConfig {
        name: "local".into(),
        protocol: friki_core::config::Protocol::LocalShell,
        local_command: Some("echo hola; exit 4".into()),
        ..SessionConfig::default()
    };
    let kh = std::env::temp_dir().join(format!("friki-tui-kh-{}", std::process::id()));
    let (res, screen) = run_tui(initial, kh, "local", &[], true, |_keys, sc| async move {
        wait_for("salida local", || sc.text().contains("hola")).await;
    })
    .await;
    assert_eq!(res.expect("sesión"), 4);
    let text = screen.text();
    assert!(text.contains("hola"), "{text}");
}

/// Directorio de pruebas de `drive` (sin sesiones extra): ver `run_tui`.
fn test_dir() -> PathBuf {
    std::env::temp_dir().join(format!("friki-tui-test-{}-0-0", std::process::id()))
}

#[tokio::test]
#[ignore = "requiere OpenSSH de pruebas"]
async fn busqueda_en_un_panel_encuentra_la_salida() {
    let Some(e) = env() else { return };
    let (res, _) = drive(&e, &[], true, |keys, sc| async move {
        wait_for("conectado", || sc.text().contains("conectado")).await;
        // La orden escrita no contiene "hola": solo la salida coincide.
        keys.typed("printf 'ho%sa\\n' l\n");
        wait_for("salida", || sc.text().lines().any(|l| l.trim() == "hola")).await;
        keys.prefixed('/');
        keys.typed("hola");
        wait_for("una coincidencia", || sc.text().contains("/hola  1/1")).await;
        keys.send(key(KeyCode::Esc));
        wait_for("búsqueda cerrada", || !sc.text().contains("/hola  1/1")).await;
        keys.prefixed('q');
    })
    .await;
    assert_eq!(res.expect("sesión"), 0);
}

#[tokio::test]
#[ignore = "requiere OpenSSH de pruebas"]
async fn un_script_envia_y_espera_la_respuesta() {
    let Some(e) = env() else { return };
    let (res, _) = drive(&e, &[], true, |keys, sc| async move {
        wait_for("conectado", || sc.text().contains("conectado")).await;
        let scripts = test_dir().join("scripts");
        std::fs::create_dir_all(&scripts).expect("dir de scripts");
        std::fs::write(
            scripts.join("ok.rhai"),
            "crt.Screen.Send(\"echo friki-$((40+2))\\n\");\n\
             if !crt.Screen.WaitForString(\"friki-42\", 10) { crt.Quit(2); }\n\
             crt.Dialog.MessageBox(\"script-listo\");\n",
        )
        .expect("script");
        keys.prefixed('r');
        wait_for("selector de scripts", || {
            sc.text().contains("Ejecutar script")
        })
        .await;
        keys.send(key(KeyCode::Enter)); // un solo script: el campo ya trae su nombre
        wait_for("fin del script", || sc.text().contains("script-listo")).await;
        assert!(sc.text().contains("friki-42"));
        keys.prefixed('q');
    })
    .await;
    assert_eq!(res.expect("sesión"), 0);
}

#[tokio::test]
#[ignore = "requiere OpenSSH de pruebas con lrzsz"]
async fn recibe_un_fichero_por_zmodem_de_extremo_a_extremo() {
    let Some(e) = env() else { return };
    let expected: String = (1..=20000).map(|i| format!("{i}\n")).collect();
    let downloads = test_dir().join("downloads");
    let _ = std::fs::remove_dir_all(&downloads);
    let (res, _) = drive(&e, &[], true, |keys, sc| async move {
        wait_for("conectado", || sc.text().contains("conectado")).await;
        keys.typed("seq 1 20000 > /tmp/zf.txt\n");
        keys.typed("command -v sz >/dev/null && sz /tmp/zf.txt || echo SZ-FALTA\n");
        wait_for("petición Zmodem o falta de sz", || {
            let t = sc.text();
            t.contains("Recibir ficheros por Zmodem") || t.lines().any(|l| l.trim() == "SZ-FALTA")
        })
        .await;
        if sc.text().contains("Recibir ficheros por Zmodem") {
            // El diálogo descarta el teclado durante 1 s y exige yes + Enter.
            tokio::time::sleep(std::time::Duration::from_millis(1200)).await;
            keys.typed("yes\n");
            wait_for("transferencia terminada", || {
                sc.text().contains("Recibidos 1 fichero")
            })
            .await;
            keys.send(key(KeyCode::Enter));
        } else {
            eprintln!("SALTADA (no ejecutada): sz no está en el contenedor");
        }
        keys.prefixed('q');
    })
    .await;
    assert_eq!(res.expect("sesión"), 0);
    if let Ok(got) = std::fs::read_to_string(downloads.join("zf.txt")) {
        assert_eq!(got, expected);
    }
}
