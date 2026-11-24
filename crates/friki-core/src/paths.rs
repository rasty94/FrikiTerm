//! Rutas de la aplicación según la plataforma (XDG, `~/Library/Application Support`, `%APPDATA%`).
//! `FRIKI_HOME` las sustituye todas (útil para pruebas).

use std::path::PathBuf;

pub fn config_dir() -> PathBuf {
    todo!("friki-core: config_dir")
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
