//! Pruebas de extremo a extremo del binario `friki` contra el sshd de pruebas.
//! `eval "$(scripts/test-sshd.sh up)"` y `cargo test -p friki-cli -- --include-ignored`.

use std::io::{BufRead, BufReader, Read};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::Duration;

struct Env {
    home: tempfile::TempDir,
    port: String,
    user: String,
    key: String,
    host: String,
    tag: String,
}

fn var(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| panic!("falta {name}: ejecuta scripts/test-sshd.sh up"))
}

fn env(tag: &str) -> Env {
    let home = tempfile::tempdir().expect("tempdir");
    std::fs::copy(
        var("FRIKI_TEST_KNOWN_HOSTS"),
        home.path().join("known_hosts"),
    )
    .expect("known_hosts");
    Env {
        home,
        port: var("FRIKI_TEST_PORT"),
        user: var("FRIKI_TEST_USER"),
        key: var("FRIKI_TEST_KEY"),
        host: var("FRIKI_TEST_HOST"),
        tag: format!("e2e-{tag}-{}", std::process::id()),
    }
}

impl Env {
    fn cmd(&self, args: &[&str]) -> Command {
        let mut c = Command::new(env!("CARGO_BIN_EXE_friki"));
        c.env("FRIKI_HOME", self.home.path())
            .env_remove("SSH_AUTH_SOCK")
            .arg(args[0])
            // Antes del resto: `exec` traga como comando todo lo que sigue al destino.
            .args(["-p", &self.port, "-l", &self.user, "-i", &self.key])
            .args(&args[1..])
            .stdin(Stdio::null());
        c
    }

    fn run(&self, args: &[&str]) -> Output {
        self.cmd(args).output().expect("ejecutar friki")
    }

    fn ok(&self, args: &[&str]) -> String {
        let o = self.run(args);
        assert!(
            o.status.success(),
            "{args:?}: {:?}\n{}",
            o.status.code(),
            String::from_utf8_lossy(&o.stderr)
        );
        String::from_utf8_lossy(&o.stdout).into_owned()
    }

    /// `host:ruta` remota bajo un directorio propio de esta prueba.
    fn remote(&self, rel: &str) -> String {
        format!("{}:{}/{rel}", self.host, self.tag)
    }

    fn exec(&self, cmd: &str) -> Output {
        self.run(&["exec", &self.host, cmd])
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        let _ = self.exec(&format!("rm -rf {}", self.tag));
    }
}

fn write(p: &Path, text: &str) {
    std::fs::create_dir_all(p.parent().expect("padre")).expect("mkdir");
    std::fs::write(p, text).expect("escribir");
}

fn s(p: &Path) -> String {
    p.to_str().expect("utf8").to_string()
}

#[test]
#[ignore = "necesita el sshd de pruebas"]
fn ls_get_put() {
    let e = env("xfer");
    e.ok(&["exec", &e.host, &format!("mkdir -p {}", e.tag)]);
    let work = tempfile::tempdir().expect("tempdir");
    let a = work.path().join("a.txt");
    write(&a, "hola");

    e.ok(&["put", &s(&a), &format!("{}:{}/", e.host, e.tag)]);
    let listing = e.ok(&["ls", &format!("{}:{}", e.host, e.tag), "--json"]);
    let v: serde_json::Value = serde_json::from_str(&listing).expect("json");
    assert_eq!(v[0]["name"], "a.txt");
    assert_eq!(v[0]["size"], 4);

    let back = work.path().join("back.txt");
    e.ok(&["get", &e.remote("a.txt"), &s(&back)]);
    assert_eq!(std::fs::read_to_string(&back).expect("leer"), "hola");

    // cp por SFTP hacia un directorio local y progreso JSON.
    let out = e.ok(&["cp", &e.remote("a.txt"), &s(work.path()), "--json"]);
    assert!(out.contains("\"event\":\"done\""), "{out}");

    // Un fichero que no existe: código 254, no 0.
    let o = e.run(&["get", &e.remote("no-existe"), &s(&work.path().join("x"))]);
    assert_eq!(o.status.code(), Some(254));
}

#[test]
#[ignore = "necesita el sshd de pruebas"]
fn sync_dry_run_y_real() {
    let e = env("sync");
    let work = tempfile::tempdir().expect("tempdir");
    let dir = work.path().join("src");
    write(&dir.join("x.txt"), "uno");
    write(&dir.join("sub/y.txt"), "dos");
    write(&dir.join("skip.log"), "no");
    let (d, r) = (s(&dir), format!("{}:{}", e.host, e.tag));

    let plan = e.ok(&["sync", &d, &r, "--dry-run", "--json", "--exclude", "*.log"]);
    let paths: Vec<String> = plan
        .lines()
        .map(|l| {
            let v: serde_json::Value = serde_json::from_str(l).expect("json");
            format!(
                "{} {}",
                v["action"].as_str().unwrap_or(""),
                v["path"].as_str().unwrap_or("")
            )
        })
        .collect();
    assert!(paths.contains(&"upload x.txt".to_string()), "{paths:?}");
    assert!(paths.contains(&"upload sub/y.txt".to_string()), "{paths:?}");
    assert!(!paths.iter().any(|p| p.contains("skip.log")), "{paths:?}");
    // El dry-run no creó nada.
    assert_eq!(e.exec(&format!("test -e {}", e.tag)).status.code(), Some(1));

    e.ok(&["sync", &d, &r, "--exclude", "*.log"]);
    let cat = e.exec(&format!("cat {0}/x.txt {0}/sub/y.txt", e.tag));
    assert_eq!(String::from_utf8_lossy(&cat.stdout), "unodos");
    // Idempotente: el segundo plan está vacío.
    assert_eq!(
        e.ok(&["sync", &d, &r, "--dry-run", "--exclude", "*.log"]),
        ""
    );

    // Bajada a otro directorio.
    let down = work.path().join("down");
    e.ok(&["sync", &s(&down), &r, "--down"]);
    assert_eq!(
        std::fs::read_to_string(down.join("sub/y.txt")).expect("leer"),
        "dos"
    );
}

#[test]
#[ignore = "necesita el sshd de pruebas"]
fn pubkey_install_es_idempotente() {
    let e = env("pubkey");
    let work = tempfile::tempdir().expect("tempdir");
    let key = work.path().join("nueva");
    let o = Command::new(env!("CARGO_BIN_EXE_friki"))
        .args([
            "keygen",
            "-f",
            &s(&key),
            "-C",
            "e2e-pubkey",
            "--no-passphrase",
        ])
        .output()
        .expect("keygen");
    assert!(o.status.success());
    let pubfile = PathBuf::from(format!("{}.pub", key.display()));
    let pubtext = std::fs::read_to_string(&pubfile).expect("pub");
    let blob = pubtext
        .split_whitespace()
        .nth(1)
        .expect("base64")
        .to_string();

    // `-I` autentica con la clave de pruebas; `-i` es la que se instala.
    let install = |e: &Env| {
        let o = Command::new(env!("CARGO_BIN_EXE_friki"))
            .env("FRIKI_HOME", e.home.path())
            .env_remove("SSH_AUTH_SOCK")
            .args(["pubkey-install", &e.host, "-i", &s(&pubfile), "-I", &e.key])
            .args(["-p", &e.port, "-l", &e.user])
            .stdin(Stdio::null())
            .output()
            .expect("pubkey-install");
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
        String::from_utf8_lossy(&o.stdout).into_owned()
    };
    assert!(install(&e).contains("instalada"));
    assert!(install(&e).contains("ya estaba"));
    let n = e.exec(&format!("grep -c {blob} ~/.ssh/authorized_keys"));
    assert_eq!(String::from_utf8_lossy(&n.stdout).trim(), "1");
    let mode = e.exec("stat -c %a ~/.ssh ~/.ssh/authorized_keys");
    assert_eq!(String::from_utf8_lossy(&mode.stdout).trim(), "700\n600");

    // Se puede entrar con la clave nueva.
    let o = Command::new(env!("CARGO_BIN_EXE_friki"))
        .env("FRIKI_HOME", e.home.path())
        .env_remove("SSH_AUTH_SOCK")
        .args([
            "exec",
            "-i",
            &s(&key),
            "-p",
            &e.port,
            "-l",
            &e.user,
            &e.host,
            "true",
        ])
        .stdin(Stdio::null())
        .output()
        .expect("exec");
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));

    // Deja el servidor como estaba.
    e.exec(&format!("sed -i '/{blob}/d' ~/.ssh/authorized_keys"));
}

#[test]
#[ignore = "necesita el sshd de pruebas"]
fn script_devuelve_su_codigo() {
    let e = env("script");
    let work = tempfile::tempdir().expect("tempdir");
    let sc = work.path().join("demo.rhai");
    write(
        &sc,
        r#"
        crt.Screen.Send("echo friki-$((1+2))\n");
        if !crt.Screen.WaitForString("friki-3", 15) { crt.Quit(90); }
        crt.Quit(3);
        "#,
    );
    let o = e.run(&["script", &s(&sc), "--target", &e.host]);
    assert_eq!(
        o.status.code(),
        Some(3),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
}

#[test]
#[ignore = "necesita el sshd de pruebas"]
fn fwd_local_llega_al_banner_ssh() {
    let e = env("fwd");
    let mut child = e
        .cmd(&["fwd", &e.host, "-L", &format!("0:127.0.0.1:{}", e.port)])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("fwd");
    let mut line = String::new();
    let out = child.stdout.take().expect("stdout");
    BufReader::new(out).read_line(&mut line).expect("línea");
    let addr = line
        .split("escuchando en ")
        .nth(1)
        .map(|r| r.trim().trim_end_matches(')').to_string());
    let banner = addr.map(|a| {
        let mut s = TcpStream::connect(&a).expect("conectar a la redirección");
        s.set_read_timeout(Some(Duration::from_secs(10)))
            .expect("timeout");
        let mut b = [0u8; 8];
        s.read_exact(&mut b).expect("banner");
        b
    });
    let _ = child.kill();
    let _ = child.wait();
    assert_eq!(
        banner.as_ref().map(|b| &b[..8]),
        Some(&b"SSH-2.0-"[..]),
        "{line}"
    );
}

#[test]
fn sin_destino_remoto_es_error_de_uso() {
    let o = Command::new(env!("CARGO_BIN_EXE_friki"))
        .args(["ls", "sin-dos-puntos"])
        .output()
        .expect("friki");
    assert_eq!(o.status.code(), Some(250));
}
