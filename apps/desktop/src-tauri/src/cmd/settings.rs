//! Preferencias de escritorio (`desktop.toml`) y apertura de enlaces en el navegador del SO.

use std::fs;
use std::io::Write;
use std::path::Path;

use crate::dto::DesktopSettings;
use crate::error::{CmdError, ErrorKind};

const MAX_FILE: u64 = 64 * 1024;
const MAX_URL: usize = 2048;

fn file() -> std::path::PathBuf {
    friki_core::paths::config_dir().join("desktop.toml")
}

pub fn validate(s: &DesktopSettings) -> Result<(), CmdError> {
    if !(8..=32).contains(&s.font_size) {
        return Err(CmdError::invalid("tamaño de fuente fuera de rango (8-32)"));
    }
    if s.scrollback > 100_000 {
        return Err(CmdError::invalid("scrollback fuera de rango (0-100000)"));
    }
    if s.font_family.len() > 200 || s.font_family.chars().any(char::is_control) {
        return Err(CmdError::invalid("familia de fuente no válida"));
    }
    Ok(())
}

/// Falta el fichero, no se puede leer o no valida: valores por defecto (nunca bloquea la app).
pub fn load(path: &Path) -> DesktopSettings {
    let Ok(meta) = fs::metadata(path) else {
        return DesktopSettings::default();
    };
    if meta.len() > MAX_FILE {
        return DesktopSettings::default();
    }
    fs::read_to_string(path)
        .ok()
        .and_then(|t| toml::from_str::<DesktopSettings>(&t).ok())
        .filter(|s| validate(s).is_ok())
        .unwrap_or_default()
}

/// Escritura atómica: temporal junto al destino y `rename`.
pub fn store(path: &Path, s: &DesktopSettings) -> Result<(), CmdError> {
    validate(s)?;
    let text =
        toml::to_string_pretty(s).map_err(|e| CmdError::new(ErrorKind::Other, e.to_string()))?;
    let dir = path.parent().unwrap_or(Path::new("."));
    fs::create_dir_all(dir)?;
    let tmp = path.with_extension("toml.tmp");
    let mut f = fs::File::create(&tmp)?;
    f.write_all(text.as_bytes())?;
    f.sync_all()?;
    fs::rename(&tmp, path)?;
    Ok(())
}

#[tauri::command]
pub async fn settings_get() -> Result<DesktopSettings, CmdError> {
    Ok(load(&file()))
}

#[tauri::command]
pub async fn settings_save(settings: DesktopSettings) -> Result<(), CmdError> {
    store(&file(), &settings)
}

/// Solo http/https, una línea, sin espacios ni caracteres de control.
pub fn check_url(url: &str) -> Result<(), CmdError> {
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"));
    match rest {
        Some(r)
            if !r.is_empty()
                && url.len() <= MAX_URL
                && !url.chars().any(|c| c.is_control() || c.is_whitespace()) =>
        {
            Ok(())
        }
        _ => Err(CmdError::invalid("solo se abren enlaces http(s)")),
    }
}

#[tauri::command]
pub async fn open_url(url: String) -> Result<(), CmdError> {
    check_url(&url)?;
    #[cfg(target_os = "macos")]
    let mut cmd = std::process::Command::new("open");
    #[cfg(target_os = "windows")]
    let mut cmd = {
        // rundll32 evita pasar la URL por cmd.exe (metacaracteres como `&`).
        let mut c = std::process::Command::new("rundll32");
        c.arg("url.dll,FileProtocolHandler");
        c
    };
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let mut cmd = std::process::Command::new("xdg-open");
    cmd.arg(&url)
        .spawn()
        .map(|mut c| {
            // Se recoge en segundo plano para no dejar zombis.
            std::thread::spawn(move || {
                let _ = c.wait();
            });
        })
        .map_err(|e| {
            CmdError::new(
                ErrorKind::Other,
                format!("no se pudo abrir el navegador: {e}"),
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dto::{CursorStyle, RightClick, ThemePref};

    #[test]
    fn missing_file_gives_defaults() {
        let d = tempfile::tempdir().unwrap();
        assert_eq!(
            load(&d.path().join("nope.toml")),
            DesktopSettings::default()
        );
    }

    #[test]
    fn roundtrip_and_atomic_overwrite() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("sub/desktop.toml");
        let s = DesktopSettings {
            theme: ThemePref::Dark,
            font_size: 16,
            cursor_style: CursorStyle::Bar,
            right_click: RightClick::Paste,
            copy_on_select: true,
            ..Default::default()
        };
        store(&p, &s).unwrap();
        assert_eq!(load(&p), s);
        store(&p, &DesktopSettings::default()).unwrap();
        assert_eq!(load(&p), DesktopSettings::default());
        assert_eq!(fs::read_dir(p.parent().unwrap()).unwrap().count(), 1);
    }

    #[test]
    fn unknown_and_missing_fields() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("d.toml");
        fs::write(&p, "futuro = 1\nfontSize = 20\n").unwrap();
        let s = load(&p);
        assert_eq!(s.font_size, 20);
        assert_eq!(s.scrollback, 10_000);
    }

    #[test]
    fn invalid_values_rejected_and_ignored_on_load() {
        let bad = DesktopSettings {
            font_size: 99,
            ..Default::default()
        };
        assert!(validate(&bad).is_err());
        let bad = DesktopSettings {
            scrollback: 1_000_000,
            ..Default::default()
        };
        assert!(store(&std::env::temp_dir().join("never.toml"), &bad).is_err());
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("d.toml");
        fs::write(&p, "fontSize = 2\n").unwrap();
        assert_eq!(load(&p), DesktopSettings::default());
        fs::write(&p, "theme = \"neon\"\n").unwrap();
        assert_eq!(load(&p), DesktopSettings::default());
    }

    #[test]
    fn url_whitelist() {
        assert!(check_url("https://example.com/a?b=1&c=2").is_ok());
        assert!(check_url("http://localhost:8080").is_ok());
        for bad in [
            "file:///etc/passwd",
            "javascript:alert(1)",
            "https://",
            "ssh://h",
            "https://a b",
            "https://a\nb",
            "mailto:a@b",
        ] {
            assert!(check_url(bad).is_err(), "{bad}");
        }
    }
}
