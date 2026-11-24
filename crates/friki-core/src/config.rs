//! Sesiones guardadas en TOML (una por fichero) e importación de `~/.ssh/config`.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::{Error, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AuthMethod {
    Agent,
    PublicKey,
    KeyboardInteractive,
    Password,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LogConfig {
    /// Admite `%S` (sesión), `%H` (host) y `%D` (fecha AAAA-MM-DD).
    pub path: PathBuf,
    pub append: bool,
    /// Registrar también lo que teclea el usuario. Aun así, la entrada tras un
    /// aviso de contraseña nunca se escribe (TODO.md §3).
    pub log_input: bool,
}

impl Default for LogConfig {
    fn default() -> Self {
        Self {
            path: PathBuf::from("%S-%D.log"),
            append: true,
            log_input: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SessionConfig {
    pub name: String,
    pub host: String,
    pub port: u16,
    /// `None` = usuario local.
    pub user: Option<String>,
    /// Orden de intento.
    pub auth: Vec<AuthMethod>,
    pub identity_files: Vec<PathBuf>,
    pub proxy_jump: Option<String>,
    /// Solo tiene efecto si friki-ssh se compila con la feature `legacy`.
    pub legacy_algorithms: bool,
    pub term: String,
    /// Intentos máximos sumando todos los métodos (TODO.md §2).
    pub max_auth_attempts: u32,
    pub connect_timeout_secs: u64,
    pub keepalive_secs: Option<u64>,
    pub log: Option<LogConfig>,
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            name: String::new(),
            host: String::new(),
            port: 22,
            user: None,
            auth: vec![
                AuthMethod::Agent,
                AuthMethod::PublicKey,
                AuthMethod::KeyboardInteractive,
                AuthMethod::Password,
            ],
            identity_files: Vec::new(),
            proxy_jump: None,
            legacy_algorithms: false,
            term: "xterm-256color".into(),
            max_auth_attempts: 3,
            connect_timeout_secs: 15,
            keepalive_secs: Some(30),
            log: None,
        }
    }
}

/// Lee todos los `*.toml` de `dir` (inexistente = lista vacía), ordenados por nombre.
pub fn load_sessions(dir: &Path) -> Result<Vec<SessionConfig>> {
    let rd = match std::fs::read_dir(dir) {
        Ok(rd) => rd,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e.into()),
    };
    let mut files = Vec::new();
    for entry in rd {
        let p = entry?.path();
        if p.extension().is_some_and(|e| e == "toml") && p.is_file() {
            files.push(p);
        }
    }
    files.sort();
    let mut out = Vec::with_capacity(files.len());
    for p in files {
        let text = std::fs::read_to_string(&p)?;
        let mut s: SessionConfig = toml::from_str(&text).map_err(|e| Error::Parse {
            path: p.display().to_string(),
            msg: e.to_string(),
        })?;
        if s.name.is_empty() {
            s.name = p
                .file_stem()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
        }
        out.push(s);
    }
    Ok(out)
}

/// Escribe `<dir>/<name>.toml` de forma atómica y devuelve la ruta.
pub fn save_session(dir: &Path, session: &SessionConfig) -> Result<PathBuf> {
    let name = &session.name;
    let bad = name.is_empty() || name == "." || name == ".." || name.contains(['/', '\\', '\0']);
    if bad {
        return Err(Error::Parse {
            path: name.clone(),
            msg: "nombre de sesión no válido como nombre de fichero".into(),
        });
    }
    let path = dir.join(format!("{name}.toml"));
    let text = toml::to_string_pretty(session).map_err(|e| Error::Parse {
        path: path.display().to_string(),
        msg: e.to_string(),
    })?;
    crate::fsutil::write_atomic(&path, text.as_bytes())?;
    Ok(path)
}

fn expand_tilde(p: &str) -> PathBuf {
    let home = || directories::BaseDirs::new().map(|d| d.home_dir().to_path_buf());
    match p.strip_prefix('~') {
        Some("") => home().unwrap_or_else(|| PathBuf::from(p)),
        Some(rest) if rest.starts_with('/') => home().map_or_else(
            || PathBuf::from(p),
            |h| h.join(rest.trim_start_matches('/')),
        ),
        _ => PathBuf::from(p),
    }
}

/// `Clave valor` o `Clave=valor`, con comillas opcionales en el valor.
fn split_directive(line: &str) -> Option<(String, String)> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return None;
    }
    let i = line.find(|c: char| c.is_whitespace() || c == '=')?;
    let key = line[..i].to_ascii_lowercase();
    let val = line[i..]
        .trim_start_matches(|c: char| c.is_whitespace() || c == '=')
        .trim();
    let val = val
        .strip_prefix('"')
        .and_then(|v| v.strip_suffix('"'))
        .unwrap_or(val);
    (!val.is_empty()).then(|| (key, val.to_string()))
}

/// Convierte los bloques `Host` concretos (sin comodines) de un `ssh_config`.
/// Soporta HostName, User, Port, IdentityFile (con `~`) y ProxyJump.
/// `Include`, `Match`, bloques con comodines y el resto de directivas se ignoran.
pub fn import_ssh_config(path: &Path) -> Result<Vec<SessionConfig>> {
    let text = String::from_utf8_lossy(&std::fs::read(path)?).into_owned();
    let mut out: Vec<SessionConfig> = Vec::new();
    // Índices en `out` del bloque abierto (un `Host a b` crea varias sesiones).
    let mut current: Vec<usize> = Vec::new();
    let mut seen: Vec<std::collections::HashSet<&'static str>> = Vec::new();
    for line in text.lines() {
        let Some((key, val)) = split_directive(line) else {
            continue;
        };
        match key.as_str() {
            "host" => {
                current.clear();
                for h in val.split_whitespace() {
                    if h.contains(['*', '?', '!']) {
                        continue;
                    }
                    current.push(out.len());
                    out.push(SessionConfig {
                        name: h.into(),
                        host: h.into(),
                        ..Default::default()
                    });
                    seen.push(Default::default());
                }
            }
            "match" => current.clear(),
            _ => {
                for &i in &current {
                    // ssh_config: el primer valor obtenido gana.
                    let (s, seen) = (&mut out[i], &mut seen[i]);
                    match key.as_str() {
                        "hostname" if seen.insert("hostname") => s.host = val.clone(),
                        "user" if seen.insert("user") => s.user = Some(val.clone()),
                        "port" if !seen.contains("port") => {
                            if let Ok(p) = val.parse() {
                                s.port = p;
                                seen.insert("port");
                            }
                        }
                        "identityfile" => s.identity_files.push(expand_tilde(&val)),
                        "proxyjump" if seen.insert("proxyjump") => s.proxy_jump = Some(val.clone()),
                        _ => {}
                    }
                }
            }
        }
    }
    Ok(out)
}

/// Resuelve un nombre de sesión guardada o `[user@]host[:port]`.
/// `None` si no es ni una sesión ni un destino válido.
pub fn resolve_target(sessions: &[SessionConfig], target: &str) -> Option<SessionConfig> {
    if let Some(s) = sessions.iter().find(|s| s.name == target) {
        return Some(s.clone());
    }
    let (user, hostport) = match target.rsplit_once('@') {
        Some((u, h)) if !u.is_empty() => (Some(u.to_string()), h),
        Some(_) => return None,
        None => (None, target),
    };
    let (host, port) = if let Some(rest) = hostport.strip_prefix('[') {
        let (h, after) = rest.split_once(']')?;
        let port = match after {
            "" => None,
            _ => Some(after.strip_prefix(':')?),
        };
        (h, port)
    } else if hostport.matches(':').count() == 1 {
        let (h, p) = hostport.split_once(':')?;
        (h, Some(p))
    } else {
        // Sin dos puntos, o IPv6 sin corchetes (sin puerto posible).
        (hostport, None)
    };
    if host.is_empty() || host.contains(char::is_whitespace) || host.contains(['@', '[', ']']) {
        return None;
    }
    let port = match port {
        None => 22,
        Some(p) => p.parse::<u16>().ok().filter(|&p| p != 0)?,
    };
    Some(SessionConfig {
        name: host.into(),
        host: host.into(),
        port,
        user,
        ..Default::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(name: &str) -> SessionConfig {
        SessionConfig {
            name: name.into(),
            host: format!("{name}.example"),
            ..Default::default()
        }
    }

    #[test]
    fn load_missing_dir_is_empty() {
        let d = tempfile::tempdir().unwrap();
        assert!(load_sessions(&d.path().join("nope")).unwrap().is_empty());
    }

    #[test]
    fn save_then_load_sorted_roundtrip() {
        let d = tempfile::tempdir().unwrap();
        let dir = d.path().join("sessions");
        let mut b = s("b");
        b.user = Some("root".into());
        b.log = Some(LogConfig::default());
        b.identity_files = vec![PathBuf::from("/k/id")];
        let pb = save_session(&dir, &b).unwrap();
        assert_eq!(pb, dir.join("b.toml"));
        save_session(&dir, &s("a")).unwrap();
        std::fs::write(dir.join("notes.txt"), "ignorar").unwrap();
        // Sobrescribir es válido.
        save_session(&dir, &b).unwrap();
        let got = load_sessions(&dir).unwrap();
        assert_eq!(got, vec![s("a"), b]);
        assert_eq!(
            std::fs::read_dir(&dir).unwrap().count(),
            3,
            "sin temporales"
        );
    }

    #[test]
    fn load_partial_toml_uses_defaults_and_stem_name() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("x.toml"), "host = \"h\"\nport = 2200\n").unwrap();
        let got = load_sessions(d.path()).unwrap();
        assert_eq!(
            (got[0].name.as_str(), got[0].host.as_str(), got[0].port),
            ("x", "h", 2200)
        );
        assert_eq!(got[0].max_auth_attempts, 3);
    }

    #[test]
    fn load_invalid_toml_is_parse_error() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("bad.toml"), "port = \"no\"").unwrap();
        assert!(matches!(load_sessions(d.path()), Err(Error::Parse { .. })));
    }

    #[test]
    fn save_rejects_path_traversal_names() {
        let d = tempfile::tempdir().unwrap();
        for n in ["", "..", "a/b", "a\\b"] {
            assert!(save_session(d.path(), &s(n)).is_err(), "{n:?}");
        }
    }

    #[test]
    fn import_ssh_config_basic() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("config");
        std::fs::write(
            &p,
            r#"
# global
Include ~/.ssh/conf.d/*
ServerAliveInterval 60

Host web
    HostName 10.0.0.5
    User deploy
    Port 2222
    IdentityFile ~/.ssh/id_web
    IdentityFile "/abs/key"
    ProxyJump bastion
    ForwardAgent yes
    HostName ignored.later
    Port 99

Host db cache
    HostName=db.internal

Host *.wild
    User nobody

Host *
    User everyone

Match host foo
    User matched

Host bare
Host late
    Port nope
    Port 2200
"#,
        )
        .unwrap();
        let got = import_ssh_config(&p).unwrap();
        let names: Vec<_> = got.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["web", "db", "cache", "bare", "late"]);
        let web = &got[0];
        assert_eq!((web.host.as_str(), web.port), ("10.0.0.5", 2222));
        assert_eq!(web.user.as_deref(), Some("deploy"));
        assert_eq!(web.proxy_jump.as_deref(), Some("bastion"));
        assert_eq!(web.identity_files.len(), 2);
        assert!(!web.identity_files[0].to_string_lossy().starts_with('~'));
        assert!(web.identity_files[0].ends_with(".ssh/id_web"));
        assert_eq!(web.identity_files[1], PathBuf::from("/abs/key"));
        assert_eq!(
            (got[1].host.as_str(), got[2].host.as_str()),
            ("db.internal", "db.internal")
        );
        assert_eq!(
            (got[3].host.as_str(), got[3].port, got[3].user.clone()),
            ("bare", 22, None)
        );
        assert_eq!(got[4].port, 2200);
    }

    #[test]
    fn import_ssh_config_missing_file_errors_and_empty_ok() {
        let d = tempfile::tempdir().unwrap();
        assert!(import_ssh_config(&d.path().join("nope")).is_err());
        let p = d.path().join("empty");
        std::fs::write(&p, "").unwrap();
        assert!(import_ssh_config(&p).unwrap().is_empty());
    }

    #[test]
    fn resolve_saved_session_wins() {
        let mut saved = s("prod");
        saved.port = 2022;
        let got = resolve_target(&[saved.clone()], "prod").unwrap();
        assert_eq!(got, saved);
    }

    #[test]
    fn resolve_adhoc_targets() {
        let r = |t| resolve_target(&[], t);
        let h = r("example.com").unwrap();
        assert_eq!((h.host.as_str(), h.port, h.user), ("example.com", 22, None));
        let h = r("bob@example.com:2222").unwrap();
        assert_eq!(
            (h.host.as_str(), h.port, h.user.as_deref()),
            ("example.com", 2222, Some("bob"))
        );
        let h = r("[::1]:2200").unwrap();
        assert_eq!((h.host.as_str(), h.port), ("::1", 2200));
        let h = r("u@[fe80::1]").unwrap();
        assert_eq!(
            (h.host.as_str(), h.port, h.user.as_deref()),
            ("fe80::1", 22, Some("u"))
        );
        let h = r("2001:db8::1").unwrap();
        assert_eq!((h.host.as_str(), h.port), ("2001:db8::1", 22));
    }

    #[test]
    fn resolve_rejects_garbage() {
        for t in [
            "",
            "@host",
            "host:",
            "host:0",
            "host:99999",
            "host:abc",
            "[::1",
            "[::1]x",
            "a b",
            "[]:22",
        ] {
            assert!(resolve_target(&[], t).is_none(), "{t:?}");
        }
    }
}
