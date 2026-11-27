//! Metadatos de hosts que no son secretos (`config_dir/desktop-hosts.toml`): última conexión, nº de
//! conexiones y SO detectado. Los escribe solo el backend; la UI los lee con `hosts_meta`.

use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use friki_core::config::{Protocol, SessionConfig};
use friki_ssh::Connection;
use serde::{Deserialize, Serialize};

use crate::error::CmdError;

const MAX_FILE: u64 = 1 << 20;
const DETECT_TIMEOUT: Duration = Duration::from_secs(5);
/// Distros que la UI sabe pintar; el resto cae a `ID_LIKE` o a "linux".
const KNOWN: [&str; 9] = [
    "debian", "ubuntu", "alpine", "rhel", "centos", "fedora", "arch", "opensuse", "raspbian",
];

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct HostMeta {
    /// Segundos Unix de la última conexión correcta.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_connected: Option<i64>,
    /// "linux", "debian", "macos", "windows"... Ver `parse_os`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub os: Option<String>,
    pub connect_count: u32,
}

pub type MetaMap = BTreeMap<String, HostMeta>;

/// Serializa lectura-modificación-escritura dentro del proceso.
static LOCK: Mutex<()> = Mutex::new(());

fn file() -> PathBuf {
    friki_core::paths::config_dir().join("desktop-hosts.toml")
}

/// Falta, ilegible o corrupto: vacío (los metadatos nunca bloquean la app).
pub fn load(path: &Path) -> MetaMap {
    match fs::metadata(path) {
        Ok(m) if m.len() <= MAX_FILE => {}
        _ => return MetaMap::new(),
    }
    fs::read_to_string(path)
        .ok()
        .and_then(|t| toml::from_str(&t).ok())
        .unwrap_or_default()
}

/// Escritura atómica: temporal junto al destino y `rename`.
pub fn store(path: &Path, m: &MetaMap) -> Result<(), CmdError> {
    let text = toml::to_string_pretty(m)
        .map_err(|e| CmdError::new(crate::error::ErrorKind::Other, e.to_string()))?;
    fs::create_dir_all(path.parent().unwrap_or(Path::new(".")))?;
    let tmp = path.with_extension("toml.tmp");
    let mut f = fs::File::create(&tmp)?;
    f.write_all(text.as_bytes())?;
    f.sync_all()?;
    fs::rename(&tmp, path)?;
    Ok(())
}

fn update(path: &Path, f: impl FnOnce(&mut MetaMap)) {
    let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut m = load(path);
    f(&mut m);
    // ponytail: un fallo al escribir metadatos se ignora; no debe afectar a la conexión.
    let _ = store(path, &m);
}

pub fn record_connect_in(path: &Path, name: &str, now: i64) {
    update(path, |m| {
        let e = m.entry(name.to_string()).or_default();
        e.last_connected = Some(now);
        e.connect_count = e.connect_count.saturating_add(1);
    });
}

pub fn set_os_in(path: &Path, name: &str, os: &str) {
    update(path, |m| {
        m.entry(name.to_string()).or_default().os = Some(os.to_string());
    });
}

pub fn rename_in(path: &Path, from: &str, to: &str) {
    update(path, |m| {
        if let Some(v) = m.remove(from) {
            m.insert(to.to_string(), v);
        }
    });
}

pub fn remove_in(path: &Path, name: &str) {
    update(path, |m| {
        m.remove(name);
    });
}

/// Gancho de `host_save`: si cambió el nombre, los metadatos lo siguen.
pub fn on_renamed(from: &str, to: &str) {
    rename_in(&file(), from, to);
}

/// Gancho de `host_delete`.
pub fn on_deleted(name: &str) {
    remove_in(&file(), name);
}

/// Gancho de apertura correcta de un host guardado: anota la conexión y, sin bloquear, detecta el SO
/// por una orden `exec` en la misma conexión (solo SSH). Los fallos se ignoran.
pub fn on_connected(cfg: &SessionConfig, conn: Option<&Arc<Connection>>) {
    if cfg.name.is_empty() || cfg.protocol == Protocol::LocalShell {
        return;
    }
    let (path, name) = (file(), cfg.name.clone());
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64);
    record_connect_in(&path, &name, now);
    if let Some(conn) = conn.cloned() {
        tokio::spawn(async move {
            if let Some(os) = detect_os(&conn).await {
                set_os_in(&path, &name, &os);
            }
        });
    }
}

pub async fn detect_os(conn: &Connection) -> Option<String> {
    let out = tokio::time::timeout(
        DETECT_TIMEOUT,
        conn.exec("uname -s; cat /etc/os-release 2>/dev/null"),
    )
    .await
    .ok()?
    .ok()?;
    parse_os(&String::from_utf8_lossy(&out.stdout))
}

fn token_ok(s: &str, max: usize) -> bool {
    !s.is_empty()
        && s.len() <= max
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
}

/// Salida de `uname -s; cat /etc/os-release` -> "linux" | distro (`ID=`) | "macos" | "freebsd" |
/// "openbsd" | "netbsd" | "windows". `None` si no se reconoce (p. ej. un Windows sin `uname`).
pub fn parse_os(out: &str) -> Option<String> {
    let mut lines = out.lines();
    let uname = lines.next()?.trim().to_ascii_lowercase();
    if !token_ok(&uname, 40) {
        return None;
    }
    let field = |key: &str| -> Option<String> {
        out.lines()
            .filter_map(|l| l.trim().strip_prefix(key))
            .next_back()
            .map(|v| v.trim().trim_matches(['"', '\'']).to_ascii_lowercase())
    };
    Some(match uname.as_str() {
        "darwin" => "macos".into(),
        "freebsd" | "openbsd" | "netbsd" => uname,
        u if u.starts_with("mingw") || u.starts_with("msys") || u.starts_with("cygwin") => {
            "windows".into()
        }
        "linux" => {
            let id = field("ID=").filter(|s| token_ok(s, 32));
            match id {
                Some(id) if KNOWN.contains(&id.as_str()) => id,
                other => field("ID_LIKE=")
                    .and_then(|l| {
                        l.split_whitespace()
                            .find(|t| KNOWN.contains(t))
                            .map(str::to_string)
                    })
                    .or(other)
                    .unwrap_or_else(|| "linux".into()),
            }
        }
        _ => return None,
    })
}

#[tauri::command]
pub async fn hosts_meta() -> Result<MetaMap, CmdError> {
    Ok(load(&file()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parsea_uname_y_os_release() {
        let deb = "Linux\nPRETTY_NAME=\"Debian GNU/Linux 12\"\nID=debian\n";
        assert_eq!(parse_os(deb).as_deref(), Some("debian"));
        let alp = "Linux\nNAME=\"Alpine Linux\"\nID=alpine\nVERSION_ID=3.19\n";
        assert_eq!(parse_os(alp).as_deref(), Some("alpine"));
        let rocky = "Linux\nID=\"rocky\"\nID_LIKE=\"rhel centos fedora\"\n";
        assert_eq!(parse_os(rocky).as_deref(), Some("rhel"));
        let mint = "Linux\nID=linuxmint\nID_LIKE=\"ubuntu debian\"\n";
        assert_eq!(parse_os(mint).as_deref(), Some("ubuntu"));
        assert_eq!(parse_os("Linux\nID=nixos\n").as_deref(), Some("nixos"));
        assert_eq!(parse_os("Linux\n").as_deref(), Some("linux"));
        assert_eq!(parse_os("Darwin\n").as_deref(), Some("macos"));
        assert_eq!(parse_os("FreeBSD\n").as_deref(), Some("freebsd"));
        assert_eq!(parse_os("MINGW64_NT-10.0\n").as_deref(), Some("windows"));
        assert_eq!(parse_os("'uname' is not recognized as a command"), None);
        assert_eq!(parse_os(""), None);
    }

    #[test]
    fn persiste_renombra_y_borra() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("desktop-hosts.toml");
        assert!(load(&p).is_empty());
        record_connect_in(&p, "web 1", 100);
        record_connect_in(&p, "web 1", 200);
        set_os_in(&p, "web 1", "debian");
        let m = load(&p);
        assert_eq!(
            m["web 1"],
            HostMeta {
                last_connected: Some(200),
                os: Some("debian".into()),
                connect_count: 2
            }
        );
        rename_in(&p, "web 1", "web2");
        let m = load(&p);
        assert!(!m.contains_key("web 1") && m["web2"].connect_count == 2);
        assert!(!p.with_extension("toml.tmp").exists());
        remove_in(&p, "web2");
        assert!(load(&p).is_empty());
        fs::write(&p, "esto no es toml {{{").unwrap();
        assert!(load(&p).is_empty());
    }
}
