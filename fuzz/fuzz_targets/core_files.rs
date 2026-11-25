#![no_main]
use friki_core::config::{import_ssh_config_report, load_app_config, load_sessions, parse_key_bytes};
use friki_core::known_hosts::KnownHosts;
use libfuzzer_sys::fuzz_target;
use std::sync::OnceLock;

fn dir() -> &'static tempfile::TempDir {
    static D: OnceLock<tempfile::TempDir> = OnceLock::new();
    D.get_or_init(|| tempfile::tempdir().unwrap())
}

fuzz_target!(|data: &[u8]| {
    let d = dir().path();
    let f = d.join("f");
    std::fs::write(&f, data).unwrap();
    let _ = load_app_config(&f);
    let _ = import_ssh_config_report(&f);
    let _ = KnownHosts::load(&f);
    // Directorio de sesiones con un unico .toml.
    let sd = d.join("sessions");
    std::fs::create_dir_all(&sd).unwrap();
    std::fs::write(sd.join("s.toml"), data).unwrap();
    let _ = load_sessions(&sd);
    let _ = parse_key_bytes(&String::from_utf8_lossy(data));
});
