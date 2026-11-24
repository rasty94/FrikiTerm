//! Rutas de la aplicación según la plataforma (XDG, `~/Library/Application Support`, `%APPDATA%`).
//! `FRIKI_HOME` las sustituye todas (útil para pruebas).

use std::ffi::OsString;
use std::path::PathBuf;

pub fn config_dir() -> PathBuf {
    resolve_config_dir(
        std::env::var_os("FRIKI_HOME"),
        directories::ProjectDirs::from("", "", "frikiSSH").map(|d| d.config_dir().to_path_buf()),
    )
}

/// Lógica pura: `FRIKI_HOME` no vacío gana; si no, el directorio de la plataforma.
fn resolve_config_dir(friki_home: Option<OsString>, platform: Option<PathBuf>) -> PathBuf {
    if let Some(home) = friki_home.filter(|v| !v.is_empty()) {
        return PathBuf::from(home);
    }
    // ponytail: sin HOME resoluble caemos al directorio actual
    platform.unwrap_or_else(|| PathBuf::from(".friki"))
}

pub fn sessions_dir() -> PathBuf {
    config_dir().join("sessions")
}

pub fn known_hosts_file() -> PathBuf {
    config_dir().join("known_hosts")
}

pub fn logs_dir() -> PathBuf {
    config_dir().join("logs")
}

/// Configuración global (`AppConfig`).
pub fn app_config_file() -> PathBuf {
    config_dir().join("config.toml")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn friki_home_overrides_platform_dir() {
        let want = PathBuf::from("/plat/frikiSSH");
        let plat = Some(want.clone());
        assert_eq!(
            resolve_config_dir(Some("/tmp/friki-x".into()), plat.clone()),
            PathBuf::from("/tmp/friki-x")
        );
        // Vacío = como no definido.
        assert_eq!(
            resolve_config_dir(Some("".into()), plat.clone()),
            want.clone()
        );
        assert_eq!(resolve_config_dir(None, plat.clone()), want);
        assert_eq!(resolve_config_dir(None, None), PathBuf::from(".friki"));
    }

    #[test]
    fn derived_paths_hang_from_config_dir() {
        let base = config_dir();
        assert_eq!(sessions_dir(), base.join("sessions"));
        assert_eq!(known_hosts_file(), base.join("known_hosts"));
        assert_eq!(logs_dir(), base.join("logs"));
    }
}
