//! Puertos serie y selector de ficheros de identidad para el formulario de hosts.

use tauri::AppHandle;
use tauri_plugin_dialog::DialogExt;
use tokio::sync::oneshot;

use crate::cmd::sftp::home;
use crate::dto::SerialPortInfo;
use crate::error::{CmdError, ErrorKind};

/// Descripción orientativa a partir del nombre del dispositivo (sin tocar el hardware).
fn describe(name: &str) -> &'static str {
    let n = name.to_ascii_lowercase();
    if n.contains("bluetooth") {
        "Bluetooth"
    } else if n.contains("usb") || n.contains("acm") {
        "USB"
    } else if n.starts_with("com") {
        "Puerto COM"
    } else {
        ""
    }
}

/// Nombres de `friki_serial::list_ports` a la forma que espera la UI (pura: se prueba sin hardware).
pub fn to_port_infos(names: Vec<String>) -> Vec<SerialPortInfo> {
    names
        .into_iter()
        .map(|name| SerialPortInfo {
            description: describe(&name).to_owned(),
            name,
        })
        .collect()
}

/// Lista los puertos serie, igual que `friki serial list`.
#[tauri::command]
pub async fn serial_ports() -> Result<Vec<SerialPortInfo>, CmdError> {
    // La enumeración es bloqueante (sysfs / IOKit): fuera del runtime async.
    tokio::task::spawn_blocking(|| friki_serial::list_ports().map(to_port_infos))
        .await
        .map_err(|e| CmdError::new(ErrorKind::Other, e.to_string()))?
        .map_err(CmdError::from)
}

/// Diálogo de fichero para una clave privada, empezando en `~/.ssh`. `None` si se cancela.
#[tauri::command]
pub async fn pick_identity_file(app: AppHandle) -> Result<Option<String>, CmdError> {
    let start = home()?.join(".ssh");
    let (tx, rx) = oneshot::channel();
    app.dialog()
        .file()
        .set_directory(start)
        .pick_file(move |p| {
            let _ = tx.send(p.and_then(|p| p.into_path().ok()));
        });
    let picked = rx.await.ok().flatten();
    Ok(picked.map(|p| p.to_string_lossy().into_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mapea_nombres_con_descripcion_orientativa() {
        let v = to_port_infos(vec![
            "/dev/ttyUSB0".into(),
            "/dev/ttyS0".into(),
            "/dev/cu.Bluetooth-Incoming-Port".into(),
            "COM3".into(),
        ]);
        let nd = |n: &str, d: &str| SerialPortInfo {
            name: n.into(),
            description: d.into(),
        };
        assert_eq!(v[0], nd("/dev/ttyUSB0", "USB"));
        assert_eq!(v[1], nd("/dev/ttyS0", ""));
        assert_eq!(v[2], nd("/dev/cu.Bluetooth-Incoming-Port", "Bluetooth"));
        assert_eq!(v[3], nd("COM3", "Puerto COM"));
        assert!(to_port_infos(Vec::new()).is_empty());
    }
}
