//! Llavero (claves SSH) y snippets.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use russh::keys::ssh_key::{HashAlg, PrivateKey, PublicKey};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, State};
use zeroize::Zeroizing;

use crate::cmd::sftp::{home, pick};
use crate::dto::{ConnId, KeyInfo, Snippet};
use crate::error::{CmdError, ErrorKind};
use crate::state::AppState;

/// Tope para ficheros de clave y de snippets (no confiar en el tamaño).
const MAX_KEY_FILE: u64 = 64 * 1024;
const MAX_SNIPPETS_FILE: u64 = 512 * 1024;
const MAX_SNIPPETS: usize = 500;
const MAX_SNIPPET_CMD: usize = 8192;

/// Nombre de fichero de clave: sin separadores y sin `.pub` (si no, `keys_list` la vería como pública).
fn check_key_name(name: &str) -> Result<(), CmdError> {
    let ok = !name.is_empty()
        && name.len() <= 64
        && !name.starts_with('.')
        && !name.ends_with(".pub")
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'));
    if ok {
        Ok(())
    } else {
        Err(CmdError::invalid("nombre de clave no válido"))
    }
}

fn write_new(path: &Path, data: &[u8]) -> Result<(), CmdError> {
    let mut opts = fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut opts, 0o600);
    opts.open(path)?.write_all(data)?;
    Ok(())
}

/// Carpetas que se escanean: `~/.ssh` y `config_dir/keys`.
fn scan_dirs() -> Result<Vec<PathBuf>, CmdError> {
    Ok(vec![home()?.join(".ssh"), friki_core::paths::keys_dir()])
}

/// Cifrada según el propio fichero; sin privada o ilegible, `false`.
fn is_encrypted(private: &Path) -> bool {
    let small = fs::metadata(private).is_ok_and(|m| m.len() <= MAX_KEY_FILE);
    small
        && fs::read_to_string(private)
            .ok()
            .and_then(|t| PrivateKey::from_openssh(&t).ok())
            .is_some_and(|k| k.is_encrypted())
}

/// Datos públicos de un `.pub`; `None` si no es una clave pública OpenSSH.
fn key_info(pub_path: &Path) -> Option<KeyInfo> {
    if !fs::metadata(pub_path).is_ok_and(|m| m.len() <= MAX_KEY_FILE) {
        return None;
    }
    let text = fs::read_to_string(pub_path).ok()?;
    let key = PublicKey::from_openssh(text.trim()).ok()?;
    let private = pub_path.to_str()?.strip_suffix(".pub")?;
    Some(KeyInfo {
        path: pub_path.to_string_lossy().into_owned(),
        algorithm: key.algorithm().as_str().to_string(),
        comment: key.comment().to_string(),
        fingerprint: key.fingerprint(HashAlg::Sha256).to_string(),
        encrypted: is_encrypted(Path::new(private)),
    })
}

fn scan(dirs: &[PathBuf]) -> Vec<KeyInfo> {
    let mut out: Vec<KeyInfo> = dirs
        .iter()
        .filter_map(|d| fs::read_dir(d).ok())
        .flatten()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.to_str().is_some_and(|s| s.ends_with(".pub")) && p.is_file())
        .filter_map(|p| key_info(&p))
        .collect();
    out.sort_by(|a, b| a.path.cmp(&b.path));
    out
}

pub fn keys_list_op() -> Result<Vec<KeyInfo>, CmdError> {
    Ok(scan(&scan_dirs()?))
}

/// La clave a instalar tiene que venir de `keys_list`: nada de rutas libres.
fn find_key<'a>(list: &'a [KeyInfo], path: &str) -> Result<&'a KeyInfo, CmdError> {
    list.iter()
        .find(|k| k.path == path)
        .ok_or_else(|| CmdError::invalid("clave no listada en keys_list"))
}

pub fn key_generate_in(
    dir: &Path,
    name: &str,
    comment: &str,
    passphrase: Option<&str>,
) -> Result<KeyInfo, CmdError> {
    check_key_name(name)?;
    if comment.len() > 256 || comment.chars().any(char::is_control) {
        return Err(CmdError::invalid("comentario no válido"));
    }
    let path = dir.join(name);
    if path.exists() {
        return Err(CmdError::invalid("ya existe una clave con ese nombre"));
    }
    let mut pub_path = path.clone().into_os_string();
    pub_path.push(".pub");
    friki_ssh::keys::generate_ed25519(&path, comment, passphrase)?;
    key_info(Path::new(&pub_path))
        .ok_or_else(|| CmdError::new(ErrorKind::Other, "clave generada ilegible"))
}

#[tauri::command]
pub async fn keys_list(_state: State<'_, AppState>) -> Result<Vec<KeyInfo>, CmdError> {
    keys_list_op()
}

#[tauri::command]
pub async fn key_generate(
    _state: State<'_, AppState>,
    name: String,
    comment: String,
    passphrase: Option<Zeroizing<String>>,
) -> Result<KeyInfo, CmdError> {
    key_generate_in(
        &friki_core::paths::keys_dir(),
        &name,
        &comment,
        passphrase.as_ref().map(|p| p.as_str()),
    )
}

/// Copia una privada OpenSSH a `keys_dir` (0600) y deriva su `.pub`. Rechaza lo que no sea clave.
pub fn key_import_in(dir: &Path, src: &Path) -> Result<KeyInfo, CmdError> {
    let name = src
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| CmdError::invalid("nombre no válido"))?;
    check_key_name(name)?;
    let md = fs::metadata(src)?;
    if !md.is_file() || md.len() > MAX_KEY_FILE {
        return Err(CmdError::invalid("no es un fichero de clave"));
    }
    let text = Zeroizing::new(fs::read_to_string(src)?);
    let key = PrivateKey::from_openssh(text.as_str())
        .map_err(|_| CmdError::invalid("no es una clave privada OpenSSH"))?;
    let public = key
        .public_key()
        .to_openssh()
        .map_err(|_| CmdError::invalid("clave pública no derivable"))?;

    fs::create_dir_all(dir)?;
    let dst = dir.join(name);
    if dst.exists() {
        return Err(CmdError::invalid("ya existe una clave con ese nombre"));
    }
    let mut pub_path = dst.clone().into_os_string();
    pub_path.push(".pub");
    let pub_path = PathBuf::from(pub_path);
    write_new(&dst, text.as_bytes())?;
    write_new(&pub_path, format!("{public}\n").as_bytes())?;
    key_info(&pub_path).ok_or_else(|| CmdError::new(ErrorKind::Other, "clave importada ilegible"))
}

#[tauri::command]
pub async fn key_import(app: AppHandle) -> Result<Option<KeyInfo>, CmdError> {
    let Some(src) = pick(&app, false).await else {
        return Ok(None);
    };
    key_import_in(&friki_core::paths::keys_dir(), &src).map(Some)
}

/// Instala la clave pública (de `keys_list`) en el `authorized_keys` remoto. `false` = ya estaba.
#[tauri::command]
pub async fn key_install(
    state: State<'_, AppState>,
    conn: ConnId,
    key_path: String,
) -> Result<bool, CmdError> {
    key_install_op(&state, conn, &key_path).await
}

pub async fn key_install_op(
    state: &AppState,
    conn: ConnId,
    key_path: &str,
) -> Result<bool, CmdError> {
    let c = state.conn(conn)?;
    let list = keys_list_op()?;
    let key = find_key(&list, key_path)?;
    let line = friki_session::pubkey::read_pubkey(Path::new(&key.path))
        .map_err(|e| CmdError::new(ErrorKind::Other, e.to_string()))?
        .ok_or_else(|| CmdError::invalid("no es una clave pública"))?;
    friki_session::pubkey::install(&c, &line)
        .await
        .map_err(|e| CmdError::new(ErrorKind::Other, e.to_string()))
}

// ------------------------------------------------------------------ snippets

/// Formato del fichero: `[[snippet]] name = "..." command = "..."`.
#[derive(Serialize, Deserialize, Default)]
struct SnippetsFile {
    #[serde(default, rename = "snippet")]
    snippets: Vec<Snippet>,
}

fn check_snippets(list: &[Snippet]) -> Result<(), CmdError> {
    if list.len() > MAX_SNIPPETS {
        return Err(CmdError::invalid("demasiados snippets"));
    }
    let mut names = std::collections::HashSet::new();
    for s in list {
        let name = s.name.trim();
        if name.is_empty() || name.chars().count() > 64 || name.chars().any(char::is_control) {
            return Err(CmdError::invalid("nombre de snippet no válido"));
        }
        if !names.insert(name) {
            return Err(CmdError::invalid(format!("nombre duplicado: {name}")));
        }
        if s.command.trim().is_empty()
            || s.command.len() > MAX_SNIPPET_CMD
            || s.command.contains('\0')
        {
            return Err(CmdError::invalid("comando de snippet no válido"));
        }
    }
    Ok(())
}

pub fn snippets_load(path: &Path) -> Result<Vec<Snippet>, CmdError> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    if fs::metadata(path)?.len() > MAX_SNIPPETS_FILE {
        return Err(CmdError::invalid("snippets.toml demasiado grande"));
    }
    let text = fs::read_to_string(path)?;
    let file: SnippetsFile =
        toml::from_str(&text).map_err(|e| CmdError::invalid(format!("snippets.toml: {e}")))?;
    check_snippets(&file.snippets)?;
    Ok(file.snippets)
}

/// Escritura atómica: temporal 0600 junto al destino y `rename`.
pub fn snippets_store(path: &Path, list: &[Snippet]) -> Result<(), CmdError> {
    check_snippets(list)?;
    let text = toml::to_string_pretty(&SnippetsFile {
        snippets: list.to_vec(),
    })
    .map_err(|e| CmdError::new(ErrorKind::Other, e.to_string()))?;
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("toml.tmp");
    let _ = fs::remove_file(&tmp);
    write_new(&tmp, text.as_bytes())?;
    fs::rename(&tmp, path)?;
    Ok(())
}

#[tauri::command]
pub async fn snippets_list(_state: State<'_, AppState>) -> Result<Vec<Snippet>, CmdError> {
    snippets_load(&friki_core::paths::snippets_file())
}

#[tauri::command]
pub async fn snippets_save(
    _state: State<'_, AppState>,
    snippets: Vec<Snippet>,
) -> Result<(), CmdError> {
    snippets_store(&friki_core::paths::snippets_file(), &snippets)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct Tmp(PathBuf);

    impl Tmp {
        fn new(tag: &str) -> Self {
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let p = std::env::temp_dir()
                .join(format!("friki-keys-{tag}-{}-{nanos}", std::process::id()));
            fs::create_dir_all(&p).unwrap();
            Tmp(p)
        }
    }

    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn genera_lista_e_importa_claves() {
        let t = Tmp::new("gen");
        let dir = t.0.join("keys");
        let k = key_generate_in(&dir, "id_test", "yo@equipo", None).unwrap();
        assert_eq!(k.algorithm, "ssh-ed25519");
        assert_eq!(k.comment, "yo@equipo");
        assert!(k.fingerprint.starts_with("SHA256:"));
        assert!(!k.encrypted);
        let p = key_generate_in(&dir, "id_pass", "", Some("secreto")).unwrap();
        assert!(p.encrypted);
        assert!(key_generate_in(&dir, "id_test", "", None).is_err());
        assert!(key_generate_in(&dir, "../x", "", None).is_err());
        assert!(key_generate_in(&dir, "a.pub", "", None).is_err());

        // Un .pub inválido y un fichero sin .pub no aparecen.
        fs::write(dir.join("basura.pub"), "no es una clave\n").unwrap();
        fs::write(dir.join("solo_priv"), "x").unwrap();
        let list = scan(std::slice::from_ref(&dir));
        let names: Vec<_> = list
            .iter()
            .map(|k| k.path.rsplit('/').next().unwrap())
            .collect();
        assert_eq!(names, ["id_pass.pub", "id_test.pub"]);
        assert!(
            list.iter()
                .find(|k| k.path.ends_with("id_pass.pub"))
                .unwrap()
                .encrypted
        );

        // Importar: copia 0600 y deriva el .pub; un no-clave se rechaza.
        let src = dir.join("id_test");
        let imp = key_import_in(&t.0.join("imp"), &src).unwrap();
        assert!(imp.fingerprint == k.fingerprint);
        assert!(key_import_in(&t.0.join("imp"), &dir.join("id_pass.pub")).is_err());
        fs::write(t.0.join("texto"), "hola").unwrap();
        assert!(key_import_in(&t.0.join("imp"), &t.0.join("texto")).is_err());
    }

    #[test]
    fn find_key_solo_acepta_listadas() {
        let list = vec![KeyInfo {
            path: "/k/a.pub".into(),
            algorithm: "ssh-ed25519".into(),
            comment: String::new(),
            fingerprint: String::new(),
            encrypted: false,
        }];
        assert!(find_key(&list, "/k/a.pub").is_ok());
        assert!(find_key(&list, "/etc/passwd").is_err());
    }

    #[test]
    fn snippets_roundtrip_y_validacion() {
        let t = Tmp::new("snip");
        let path = t.0.join("snippets.toml");
        assert!(snippets_load(&path).unwrap().is_empty());
        let s = |n: &str, c: &str| Snippet {
            name: n.into(),
            command: c.into(),
        };
        let list = vec![s("uptime", "uptime"), s("df", "df -h\nfree -m")];
        snippets_store(&path, &list).unwrap();
        assert_eq!(snippets_load(&path).unwrap(), list);

        assert!(snippets_store(&path, &[s("", "x")]).is_err());
        assert!(snippets_store(&path, &[s("  ", "x")]).is_err());
        assert!(snippets_store(&path, &[s("a", "x"), s("a", "y")]).is_err());
        assert!(snippets_store(&path, &[s("a", "  ")]).is_err());
        assert!(snippets_store(&path, &[s("a", "x\0")]).is_err());
        let big = "x".repeat(MAX_SNIPPET_CMD + 1);
        assert!(snippets_store(&path, &[s("a", &big)]).is_err());
        // Un fallo de validación no toca el fichero bueno.
        assert_eq!(snippets_load(&path).unwrap(), list);
    }
}
