//! Rutas de la aplicación según la plataforma (XDG, `~/Library/Application Support`, `%APPDATA%`).
//! `FRIKI_HOME` las sustituye todas (útil para pruebas).

use std::path::PathBuf;

pub fn config_dir() -> PathBuf {
    if let Some(home) = std::env::var_os("FRIKI_HOME").filter(|v| !v.is_empty()) {
        return PathBuf::from(home);
    }
    match directories::ProjectDirs::from("", "", "frikiSSH") {
        Some(d) => d.config_dir().to_path_buf(),
        // ponytail: sin HOME resoluble caemos al directorio actual
        None => PathBuf::from(".friki"),
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    // Un único test: manipula una variable de entorno global.
    #[test]
    fn friki_home_overrides_everything() {
        // SAFETY: único test del crate que toca FRIKI_HOME.
        unsafe { std::env::set_var("FRIKI_HOME", "/tmp/friki-x") };
        assert_eq!(config_dir(), PathBuf::from("/tmp/friki-x"));
        assert_eq!(sessions_dir(), PathBuf::from("/tmp/friki-x/sessions"));
        assert_eq!(
            known_hosts_file(),
            PathBuf::from("/tmp/friki-x/known_hosts")
        );
        assert_eq!(logs_dir(), PathBuf::from("/tmp/friki-x/logs"));
        unsafe { std::env::remove_var("FRIKI_HOME") };
        assert!(config_dir().ends_with("frikiSSH") || config_dir().ends_with(".friki"));
    }
}
