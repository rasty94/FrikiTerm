//! Hosts guardados (TOML en `sessions_dir`), importación de `~/.ssh/config` y avisos/respuestas globales.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use tauri::State;
use tauri::ipc::Channel;

use crate::dto::{AppEvent, ImportReport, PromptAnswer, PromptId};
use crate::error::{CmdError, ErrorKind};
use crate::state::AppState;
use friki_core::config::{self, Protocol, SessionConfig};
use friki_core::paths;
use friki_xfer::safe_name;

#[tauri::command]
pub fn app_subscribe(
    state: State<'_, AppState>,
    events: Channel<AppEvent>,
) -> Result<(), CmdError> {
    state.subscribe(events);
    Ok(())
}

#[tauri::command]
pub fn prompt_answer(
    state: State<'_, AppState>,
    id: PromptId,
    answer: PromptAnswer,
) -> Result<(), CmdError> {
    state.prompts.answer(id, answer)
}

#[tauri::command]
pub async fn hosts_list() -> Result<Vec<SessionConfig>, CmdError> {
    list_in(&paths::sessions_dir())
}

#[tauri::command]
pub async fn host_save(host: SessionConfig, previous_name: Option<String>) -> Result<(), CmdError> {
    let (name, prev) = (host.name.clone(), previous_name.clone());
    save_in(&paths::sessions_dir(), host, previous_name)?;
    if let Some(p) = prev.filter(|p| *p != name) {
        super::meta::on_renamed(&p, &name);
    }
    Ok(())
}

#[tauri::command]
pub async fn host_delete(name: String) -> Result<(), CmdError> {
    delete_in(&paths::sessions_dir(), &name)?;
    super::meta::on_deleted(&name);
    Ok(())
}

#[tauri::command]
pub async fn hosts_import_ssh_config() -> Result<ImportReport, CmdError> {
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .ok_or_else(|| CmdError::new(ErrorKind::Other, "no hay directorio personal"))?;
    import_in(
        &paths::sessions_dir(),
        &PathBuf::from(home).join(".ssh").join("config"),
    )
}

// ---- Lógica (recibe el directorio: las pruebas usan uno temporal) ----

pub fn list_in(dir: &Path) -> Result<Vec<SessionConfig>, CmdError> {
    Ok(config::load_sessions(dir)?)
}

/// Busca un host guardado por nombre.
pub fn find_in(dir: &Path, name: &str) -> Result<SessionConfig, CmdError> {
    list_in(dir)?
        .into_iter()
        .find(|h| h.name == name)
        .ok_or_else(|| CmdError::new(ErrorKind::Notfound, format!("host '{name}' no encontrado")))
}

/// Reglas comunes a los hosts guardados y a las conexiones rápidas (sin nombre).
pub fn validate(h: &SessionConfig) -> Result<(), CmdError> {
    if matches!(h.protocol, Protocol::Tn3270) {
        return Err(CmdError::new(
            ErrorKind::Unsupported,
            "TN3270 no está disponible en el escritorio",
        ));
    }
    if h.port == 0 {
        return Err(CmdError::invalid("puerto no válido"));
    }
    let red = matches!(
        h.protocol,
        Protocol::Ssh | Protocol::Telnet | Protocol::TelnetTls | Protocol::Raw
    );
    if red && h.host.trim().is_empty() {
        return Err(CmdError::invalid("falta el host"));
    }
    Ok(())
}

fn check_name(name: &str) -> Result<(), CmdError> {
    if safe_name(name) {
        Ok(())
    } else {
        Err(CmdError::invalid(format!("nombre no válido: '{name}'")))
    }
}

/// Crea o actualiza un host. Si cambia el nombre, borra el fichero anterior.
pub fn save_in(
    dir: &Path,
    mut host: SessionConfig,
    previous: Option<String>,
) -> Result<(), CmdError> {
    check_name(&host.name)?;
    validate(&host)?;
    host.folder = config::normalize_folder(&host.folder)
        .ok_or_else(|| CmdError::invalid(format!("carpeta no válida: '{}'", host.folder)))?;
    if let Some(prev) = previous.as_deref().filter(|p| *p != host.name) {
        check_name(prev)?;
        if prev.eq_ignore_ascii_case(&host.name) {
            // Solo cambia la capitalización: en un sistema sin distinguir mayúsculas el fichero
            // anterior es el mismo que el nuevo (el existente no es un conflicto). Se renombra
            // primero, que vale con o sin distinguir mayúsculas, y luego se guarda encima.
            match std::fs::rename(
                dir.join(format!("{prev}.toml")),
                dir.join(format!("{}.toml", host.name)),
            ) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e.into()),
                _ => {}
            }
            config::save_session(dir, &host)?;
            return Ok(());
        }
        if dir.join(format!("{}.toml", host.name)).exists() {
            return Err(CmdError::invalid(format!(
                "la sesión '{}' ya existe",
                host.name
            )));
        }
        config::save_session(dir, &host)?;
        config::delete_session(dir, prev)?;
        return Ok(());
    }
    config::save_session(dir, &host)?;
    Ok(())
}

pub fn delete_in(dir: &Path, name: &str) -> Result<(), CmdError> {
    check_name(name)?;
    config::delete_session(dir, name)?;
    Ok(())
}

/// Importa `ssh_config` sin sobrescribir hosts existentes (como `sessions import-ssh-config`).
pub fn import_in(dir: &Path, ssh_config: &Path) -> Result<ImportReport, CmdError> {
    let (imported, mut warnings) = config::import_ssh_config_report(ssh_config)?;
    let existing: HashSet<String> = list_in(dir)?.into_iter().map(|h| h.name).collect();
    let (mut added, mut omitidas) = (0u32, 0usize);
    for h in &imported {
        if existing.contains(&h.name) {
            omitidas += 1;
            continue;
        }
        match config::save_session(dir, h) {
            Ok(_) => added += 1,
            Err(e) => warnings.push(format!("{}: {e}", h.name)),
        }
    }
    if omitidas > 0 {
        warnings.push(format!("{omitidas} omitidas por existir"));
    }
    Ok(ImportReport { added, warnings })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host(name: &str) -> SessionConfig {
        SessionConfig {
            name: name.into(),
            host: "10.0.0.1".into(),
            port: 22,
            ..Default::default()
        }
    }

    #[test]
    fn valida_nombre_puerto_host_y_protocolo() {
        let dir = tempfile::tempdir().unwrap();
        for bad in ["", "..", "a/b", "a\\b", "x:y"] {
            assert_eq!(
                save_in(dir.path(), host(bad), None).unwrap_err().kind,
                ErrorKind::Invalid,
                "{bad:?}"
            );
        }
        let mut h = host("ok");
        h.port = 0;
        assert_eq!(
            save_in(dir.path(), h, None).unwrap_err().kind,
            ErrorKind::Invalid
        );
        let mut h = host("ok");
        h.host = " ".into();
        assert_eq!(
            save_in(dir.path(), h, None).unwrap_err().kind,
            ErrorKind::Invalid
        );
        let mut h = host("ok");
        h.protocol = Protocol::Tn3270;
        assert_eq!(
            save_in(dir.path(), h, None).unwrap_err().kind,
            ErrorKind::Unsupported
        );
        assert!(list_in(dir.path()).unwrap().is_empty());
    }

    #[test]
    fn normaliza_carpeta_y_rechaza_rutas() {
        let dir = tempfile::tempdir().unwrap();
        let mut h = host("a");
        h.folder = "/clientes//acme/".into();
        save_in(dir.path(), h, None).unwrap();
        assert_eq!(list_in(dir.path()).unwrap()[0].folder, "clientes/acme");
        let mut h = host("b");
        h.folder = "clientes/../x".into();
        assert_eq!(
            save_in(dir.path(), h, None).unwrap_err().kind,
            ErrorKind::Invalid
        );
    }

    #[test]
    fn renombrar_borra_el_anterior_y_no_pisa() {
        let dir = tempfile::tempdir().unwrap();
        save_in(dir.path(), host("uno"), None).unwrap();
        save_in(dir.path(), host("otro"), None).unwrap();
        // Renombrar a un nombre ocupado: rechazo, nada se borra.
        let e = save_in(dir.path(), host("otro"), Some("uno".into())).unwrap_err();
        assert_eq!(e.kind, ErrorKind::Invalid);
        assert_eq!(list_in(dir.path()).unwrap().len(), 2);
        // Renombrar a uno libre: el anterior desaparece.
        save_in(dir.path(), host("dos"), Some("uno".into())).unwrap();
        let names: Vec<_> = list_in(dir.path())
            .unwrap()
            .into_iter()
            .map(|h| h.name)
            .collect();
        assert_eq!(names, ["dos", "otro"]);
    }

    #[test]
    fn borrar_es_idempotente_y_valida_el_nombre() {
        let dir = tempfile::tempdir().unwrap();
        save_in(dir.path(), host("uno"), None).unwrap();
        delete_in(dir.path(), "uno").unwrap();
        delete_in(dir.path(), "uno").unwrap();
        assert!(list_in(dir.path()).unwrap().is_empty());
        assert_eq!(
            delete_in(dir.path(), "../uno").unwrap_err().kind,
            ErrorKind::Invalid
        );
    }

    #[test]
    fn importa_sin_pisar() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = dir.path().join("ssh_config");
        std::fs::write(
            &cfg,
            "Host web\n  HostName 10.0.0.2\n  User root\n  Port 2222\n\nHost db\n  HostName 10.0.0.3\n",
        )
        .unwrap();
        let hosts = dir.path().join("sessions");
        let r = import_in(&hosts, &cfg).unwrap();
        assert_eq!(r.added, 2);
        let r = import_in(&hosts, &cfg).unwrap();
        assert_eq!(r.added, 0);
        assert!(r.warnings.iter().any(|w| w.contains("2 omitidas")));
        assert_eq!(find_in(&hosts, "web").unwrap().port, 2222);
    }
}
