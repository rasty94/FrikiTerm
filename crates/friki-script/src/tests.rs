#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use super::*;

#[derive(Default)]
struct Mock {
    sent: Mutex<Vec<String>>,
    incoming: Mutex<String>,
    waits: Mutex<Vec<Duration>>,
    connects: Mutex<Vec<String>>,
    msgs: Mutex<Vec<String>>,
    connected: AtomicBool,
}

impl Mock {
    fn with_input(s: &str) -> Arc<Self> {
        let m = Self::default();
        *m.incoming.lock().unwrap() = s.to_string();
        Arc::new(m)
    }

    /// Consume hasta la primera aparición de cualquier needle; devuelve (índice, texto previo).
    fn take(&self, needles: &[String]) -> Option<(usize, String)> {
        let mut buf = self.incoming.lock().unwrap();
        let (idx, pos) = needles
            .iter()
            .enumerate()
            .filter_map(|(i, n)| buf.find(n.as_str()).map(|p| (i, p)))
            .min_by_key(|&(_, p)| p)?;
        let before = buf[..pos].to_string();
        let cut = pos + needles[idx].len();
        *buf = buf[cut..].to_string();
        Some((idx, before))
    }
}

#[async_trait::async_trait]
impl ScriptHost for Mock {
    async fn send(&self, text: &str) -> Result<(), String> {
        self.sent.lock().unwrap().push(text.to_string());
        Ok(())
    }
    async fn wait_for_strings(
        &self,
        needles: &[String],
        timeout: Duration,
    ) -> Result<Option<usize>, String> {
        self.waits.lock().unwrap().push(timeout);
        Ok(self.take(needles).map(|(i, _)| i))
    }
    async fn read_string(
        &self,
        until: &[String],
        timeout: Duration,
    ) -> Result<Option<String>, String> {
        self.waits.lock().unwrap().push(timeout);
        Ok(self.take(until).map(|(_, s)| s))
    }
    fn screen_text(&self) -> Vec<String> {
        vec!["hola mundo".into(), "segunda fila".into()]
    }
    fn cursor(&self) -> (u16, u16) {
        (2, 5)
    }
    async fn connect(&self, target: &str) -> Result<(), String> {
        self.connects.lock().unwrap().push(target.to_string());
        self.connected.store(true, Ordering::SeqCst);
        Ok(())
    }
    async fn disconnect(&self) -> Result<(), String> {
        self.connected.store(false, Ordering::SeqCst);
        Ok(())
    }
    fn connected(&self) -> bool {
        self.connected.load(Ordering::SeqCst)
    }
    fn message(&self, text: &str) {
        self.msgs.lock().unwrap().push(text.to_string());
    }
}

async fn go(src: &str, m: &Arc<Mock>) -> Result<i64, ScriptError> {
    run(
        src,
        &[],
        m.clone(),
        &Permissions::default(),
        &Limits::default(),
    )
    .await
}

#[tokio::test]
async fn send_wait_and_read() {
    let m = Mock::with_input("login: root\nPassword: x\n$ ls\nfile1\n$ ");
    let r = go(
        r#"
        crt.Screen.Send("hola\r");
        if !crt.Screen.WaitForString("login:") { crt.Quit(90); }
        let i = crt.Screen.WaitForStrings(["nada", "Password:"], 2);
        if i != 2 { crt.Quit(91); }
        let s = crt.Screen.ReadString("$ ls", 1.5);
        if s != " x\n" { crt.Quit(92); }
        let t = crt.Screen.ReadString(["$ ", "zzz"]);
        if t != "\nfile1\n" { crt.Quit(93); }
        crt.Quit(7);
        "#,
        &m,
    )
    .await;
    assert_eq!(r.unwrap(), 7);
    assert_eq!(*m.sent.lock().unwrap(), ["hola\r"]);
}

#[tokio::test]
async fn waits_timeout_and_zero_index() {
    let m = Mock::with_input("");
    let l = Limits {
        default_wait: Duration::from_secs(2),
        ..Limits::default()
    };
    let r = run(
        r#"
        if crt.Screen.WaitForString("x") { crt.Quit(1); }
        if crt.Screen.WaitForString("x", 5) { crt.Quit(1); }
        if crt.Screen.WaitForString("x", 0.5) { crt.Quit(1); }
        if crt.Screen.WaitForStrings(["a", "b"]) != 0 { crt.Quit(1); }
        if crt.Screen.ReadString("x", 3) != "" { crt.Quit(1); }
        crt.Quit(4);
        "#,
        &[],
        m.clone(),
        &Permissions::default(),
        &l,
    )
    .await;
    assert_eq!(r.unwrap(), 4);
    assert_eq!(
        *m.waits.lock().unwrap(),
        [
            Duration::from_secs(2),
            Duration::from_secs(5),
            Duration::from_millis(500),
            Duration::from_secs(2),
            Duration::from_secs(3),
        ]
    );
}

#[tokio::test]
async fn screen_get_cursor_args_dialog_sleep() {
    let m = Mock::with_input("");
    let r = run(
        r#"
        if crt.Screen.Get(1, 1, 1, 4) != "hola" { crt.Quit(1); }
        if crt.Screen.Get(1, 6, 2, 8) != "mun\nda " { crt.Quit(2); }
        if crt.Screen.CurrentRow != 2 || crt.Screen.CurrentColumn != 5 { crt.Quit(3); }
        if crt.Arguments.len() != 2 || crt.Arguments[1] != "b" { crt.Quit(4); }
        if crt.Session.Connected { crt.Quit(5); }
        crt.Sleep(5);
        crt.Dialog.MessageBox("aviso");
        print("impreso");
        "#,
        &["a".into(), "b".into()],
        m.clone(),
        &Permissions::default(),
        &Limits::default(),
    )
    .await;
    assert_eq!(r.unwrap(), 0);
    assert_eq!(*m.msgs.lock().unwrap(), ["aviso", "impreso"]);
}

#[tokio::test]
async fn quit_code() {
    assert_eq!(go("crt.Quit(5);", &Mock::with_input("")).await.unwrap(), 5);
    // try/catch no puede tragarse Quit.
    let r = go("try { crt.Quit(6); } catch { }", &Mock::with_input("")).await;
    assert_eq!(r.unwrap(), 6);
}

#[tokio::test]
async fn connect_requires_permission() {
    let m = Mock::with_input("");
    let r = go(r#"crt.Session.Connect("srv");"#, &m).await;
    assert!(matches!(r, Err(ScriptError::Denied(_))), "{r:?}");
    let r = go(r#"crt.Session.Disconnect();"#, &m).await;
    assert!(matches!(r, Err(ScriptError::Denied(_))));
    assert!(m.connects.lock().unwrap().is_empty());

    let p = Permissions {
        connect: true,
        ..Permissions::default()
    };
    let r = run(
        r#"crt.Session.Connect("srv"); if !crt.Session.Connected { crt.Quit(1); } crt.Session.Disconnect();"#,
        &[],
        m.clone(),
        &p,
        &Limits::default(),
    )
    .await;
    assert_eq!(r.unwrap(), 0);
    assert_eq!(*m.connects.lock().unwrap(), ["srv"]);
    assert!(!m.connected());
}

static N: AtomicU32 = AtomicU32::new(0);

fn tmpdir() -> PathBuf {
    let d = std::env::temp_dir().join(format!(
        "friki-script-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    std::fs::create_dir_all(&d).unwrap();
    d.canonicalize().unwrap()
}

#[tokio::test]
async fn filesystem_permissions() {
    let root = tmpdir();
    let (rd, wr, other) = (root.join("r"), root.join("w"), root.join("o"));
    for d in [&rd, &wr, &other] {
        std::fs::create_dir_all(d).unwrap();
    }
    std::fs::write(rd.join("in.txt"), "dentro").unwrap();
    std::fs::write(other.join("secreto.txt"), "fuera").unwrap();
    let p = Permissions {
        read: vec![rd.clone()],
        write: vec![wr.clone()],
        connect: false,
    };
    let m = Mock::with_input("");
    let exec = |src: String| {
        let (m, p) = (m.clone(), p.clone());
        async move { run(&src, &[], m, &p, &Limits::default()).await }
    };
    let q = |x: &Path| x.to_string_lossy().replace('\\', "/");

    let r = exec(format!(
        r#"if crt.FileSystem.Read("{}") != "dentro" {{ crt.Quit(1); }}
           crt.FileSystem.Write("{}", "hola");"#,
        q(&rd.join("in.txt")),
        q(&wr.join("out.txt"))
    ))
    .await;
    assert_eq!(r.unwrap(), 0);
    assert_eq!(std::fs::read_to_string(wr.join("out.txt")).unwrap(), "hola");

    for src in [
        format!(
            r#"crt.FileSystem.Read("{}");"#,
            q(&other.join("secreto.txt"))
        ),
        format!(r#"crt.FileSystem.Read("{}/../o/secreto.txt");"#, q(&rd)),
        format!(
            r#"crt.FileSystem.Write("{}", "x");"#,
            q(&rd.join("nuevo.txt"))
        ),
        format!(r#"crt.FileSystem.Write("{}/../o/x.txt", "x");"#, q(&wr)),
        format!(r#"crt.FileSystem.Read("{}");"#, q(&wr.join("out.txt"))),
    ] {
        let r = exec(src.clone()).await;
        assert!(matches!(r, Err(ScriptError::Denied(_))), "{src}: {r:?}");
    }
    assert!(!other.join("x.txt").exists());

    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(other.join("secreto.txt"), rd.join("lnk")).unwrap();
        std::os::unix::fs::symlink(&other, wr.join("dirlnk")).unwrap();
        for src in [
            format!(r#"crt.FileSystem.Read("{}");"#, q(&rd.join("lnk"))),
            format!(
                r#"crt.FileSystem.Write("{}", "x");"#,
                q(&wr.join("dirlnk/y.txt"))
            ),
            format!(r#"crt.FileSystem.Write("{}", "x");"#, q(&wr.join("dirlnk"))),
        ] {
            let r = exec(src.clone()).await;
            assert!(matches!(r, Err(ScriptError::Denied(_))), "{src}: {r:?}");
        }
        assert!(!other.join("y.txt").exists());
    }
}

#[tokio::test]
async fn infinite_loop_hits_operation_limit() {
    let l = Limits {
        max_operations: 1000,
        ..Limits::default()
    };
    let r = run(
        "let i = 0; loop { i += 1; }",
        &[],
        Mock::with_input(""),
        &Permissions::default(),
        &l,
    )
    .await;
    assert!(matches!(r, Err(ScriptError::Limit(_))), "{r:?}");
}

#[tokio::test]
async fn runtime_limit() {
    let l = Limits {
        max_runtime: Duration::from_millis(100),
        ..Limits::default()
    };
    let r = run(
        "loop { }",
        &[],
        Mock::with_input(""),
        &Permissions::default(),
        &l,
    )
    .await;
    assert!(matches!(r, Err(ScriptError::Limit(_))), "{r:?}");
    // Una espera larga se recorta al tiempo restante y después se corta.
    let r = run(
        r#"crt.Screen.WaitForString("x", 3600); crt.Screen.Send("a");"#,
        &[],
        Mock::with_input(""),
        &Permissions::default(),
        &l,
    )
    .await;
    assert!(matches!(r, Err(ScriptError::Limit(_)) | Ok(0)), "{r:?}");
}

#[tokio::test]
async fn compile_error_has_line() {
    let r = go("let a = 1;\nlet b = 2;\nlet c = ;\n", &Mock::with_input("")).await;
    match r {
        Err(ScriptError::Compile(m)) => assert!(m.contains("línea 3"), "{m}"),
        o => panic!("{o:?}"),
    }
}

#[tokio::test]
async fn runtime_error_and_sandbox() {
    let m = Mock::with_input("");
    let r = go("let x = 1;\nlet y = x / 0;", &m).await;
    assert!(matches!(r, Err(ScriptError::Runtime(_))), "{r:?}");
    assert!(go(r#"eval("1 + 1");"#, &m).await.is_err());
    assert!(go(r#"import "os" as os;"#, &m).await.is_err());
    // No se puede reasignar `crt`.
    assert!(go("crt = 5;", &m).await.is_err());
}

#[tokio::test]
async fn bundled_examples() {
    let m = Mock::with_input("xx login: yy assword: zz $ ");
    let a: Vec<String> = vec!["alice".into(), "pw".into()];
    let r = run(
        include_str!("../examples/login.rhai"),
        &a,
        m.clone(),
        &Permissions::default(),
        &Limits::default(),
    )
    .await;
    assert_eq!(r.unwrap(), 0);
    assert_eq!(*m.sent.lock().unwrap(), ["alice\r", "pw\r"]);

    let m = Mock::with_input("a$ b$ ");
    let a: Vec<String> = vec!["ls".into(), "pwd".into()];
    let r = run(
        include_str!("../examples/run_commands.rhai"),
        &a,
        m.clone(),
        &Permissions::default(),
        &Limits::default(),
    )
    .await;
    assert_eq!(r.unwrap(), 0);
    assert_eq!(*m.sent.lock().unwrap(), ["ls\r", "pwd\r"]);

    let dir = tmpdir();
    let file = dir.join("salida.txt");
    let m = Mock::with_input("\nresultado\n$ ");
    let a: Vec<String> = vec!["ls".into(), file.to_string_lossy().into_owned()];
    let p = Permissions {
        write: vec![dir],
        ..Permissions::default()
    };
    let r = run(
        include_str!("../examples/collect_output.rhai"),
        &a,
        m,
        &p,
        &Limits::default(),
    )
    .await;
    assert_eq!(r.unwrap(), 0);
    assert_eq!(std::fs::read_to_string(file).unwrap(), "\nresultado\n");
}

async fn cancel_after(src: &'static str) -> (Result<i64, ScriptError>, Duration) {
    let m = Mock::with_input("");
    let tok = CancelToken::new();
    let t2 = tok.clone();
    let h = tokio::spawn(async move {
        run_cancellable(
            src,
            &[],
            m,
            &Permissions::default(),
            &Limits::default(),
            &t2,
        )
        .await
    });
    tokio::time::sleep(Duration::from_millis(150)).await;
    let t0 = Instant::now();
    tok.cancel();
    let r = h.await.unwrap();
    (r, t0.elapsed())
}

#[tokio::test(flavor = "multi_thread")]
async fn cancel_stops_sleep_and_compute() {
    for src in [
        "loop { crt.Sleep(1000); }",
        "let i = 0; loop { i += 1; }",
        r#"loop { crt.Screen.WaitForString("nunca", 30); }"#,
    ] {
        let (r, el) = cancel_after(src).await;
        assert!(
            matches!(&r, Err(ScriptError::Runtime(m)) if m == "cancelado"),
            "{r:?}"
        );
        assert!(el < Duration::from_millis(200), "{src}: {el:?}");
    }
}

#[cfg(unix)]
#[tokio::test]
async fn filesystem_rejects_fifo_symlink_and_big_write() {
    let root = tmpdir();
    let (rd, other) = (root.join("r"), root.join("o"));
    std::fs::create_dir_all(&rd).unwrap();
    std::fs::create_dir_all(&other).unwrap();
    std::fs::write(other.join("s.txt"), "x").unwrap();
    std::os::unix::fs::symlink(other.join("s.txt"), rd.join("ln")).unwrap();
    let fifo = std::ffi::CString::new(rd.join("ff").to_str().unwrap()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
    let p = Permissions {
        read: vec![rd.clone()],
        write: vec![rd.clone()],
        connect: false,
    };
    let m = Mock::with_input("");
    for f in ["ff", "ln"] {
        let src = format!(r#"crt.FileSystem.Read("{}");"#, rd.join(f).display());
        assert!(
            run(&src, &[], m.clone(), &p, &Limits::default())
                .await
                .is_err(),
            "{f}"
        );
    }
    let src = format!(
        r#"let s = "x"; for i in 0..25 {{ s += s; }} crt.FileSystem.Write("{}", s);"#,
        rd.join("big").display()
    );
    let lim = Limits {
        max_operations: u64::MAX,
        ..Limits::default()
    };
    assert!(run(&src, &[], m, &p, &lim).await.is_err());
}
