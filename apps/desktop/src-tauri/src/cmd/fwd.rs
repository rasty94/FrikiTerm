//! Redirecciones de puertos sobre una conexión abierta.

use std::net::IpAddr;
use std::sync::{MutexGuard, PoisonError};

use tauri::State;

use crate::dto::{AppEvent, ConnId, ForwardId, ForwardStatus};
use crate::error::{CmdError, ErrorKind};
use crate::state::AppState;
use friki_core::config::Forward;

fn check_host(host: &str) -> Result<(), CmdError> {
    if host.is_empty()
        || host.len() > 255
        || host
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || c == '/')
    {
        return Err(CmdError::invalid("host de destino no válido"));
    }
    Ok(())
}

/// Valida la especificación. Devuelve el aviso si el bind no es de loopback.
/// Puerto de escucha 0 vale (puerto efímero, lo dice la respuesta).
fn check_forward(spec: &Forward) -> Result<Option<String>, CmdError> {
    let bind = match spec {
        Forward::Local {
            bind,
            host,
            host_port,
            ..
        }
        | Forward::Remote {
            bind,
            host,
            host_port,
            ..
        } => {
            check_host(host)?;
            if *host_port == 0 {
                return Err(CmdError::invalid("puerto de destino no válido"));
            }
            bind
        }
        Forward::Dynamic { bind, .. } => bind,
    };
    let bind_ip = match bind.as_str() {
        "" | "localhost" => None,
        s => Some(
            s.parse::<IpAddr>()
                .map_err(|_| CmdError::invalid("dirección de escucha no válida"))?,
        ),
    };
    Ok(match bind_ip {
        Some(ip) if !ip.is_loopback() => Some(format!(
            "aviso: escucha en {ip}; otras máquinas de la red podrán usar la redirección"
        )),
        _ => None,
    })
}

fn lock<T>(m: &std::sync::Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

pub fn fwd_list_op(state: &AppState, conn: ConnId) -> Result<Vec<ForwardStatus>, CmdError> {
    state.conn(conn)?;
    Ok(lock(&state.fwds)
        .get(&conn)
        .map(|v| v.iter().map(|(s, _)| s.clone()).collect())
        .unwrap_or_default())
}

pub async fn fwd_start_op(
    state: &AppState,
    conn: ConnId,
    spec: Forward,
) -> Result<ForwardStatus, CmdError> {
    let warning = check_forward(&spec)?;
    let handle = state.conn(conn)?.start_forward(&spec).await?;
    let status = ForwardStatus {
        id: handle.id,
        spec,
        listen: handle.listen.to_string(),
        active: true,
        error: warning,
    };
    lock(&state.fwds)
        .entry(conn)
        .or_default()
        .push((status.clone(), handle));
    state.emit(AppEvent::Forward {
        conn,
        status: status.clone(),
    });
    Ok(status)
}

pub async fn fwd_stop_op(state: &AppState, conn: ConnId, id: ForwardId) -> Result<(), CmdError> {
    let c = state.conn(conn)?;
    let (mut status, handle) = {
        let mut m = lock(&state.fwds);
        let v = m
            .get_mut(&conn)
            .ok_or_else(|| CmdError::new(ErrorKind::Notfound, "redirección desconocida"))?;
        let i = v
            .iter()
            .position(|(s, _)| s.id == id)
            .ok_or_else(|| CmdError::new(ErrorKind::Notfound, "redirección desconocida"))?;
        v.remove(i)
    };
    c.stop_forward(handle).await?;
    status.active = false;
    status.error = None;
    state.emit(AppEvent::Forward { conn, status });
    Ok(())
}

#[tauri::command]
pub async fn fwd_list(
    state: State<'_, AppState>,
    conn: ConnId,
) -> Result<Vec<ForwardStatus>, CmdError> {
    fwd_list_op(&state, conn)
}

#[tauri::command]
pub async fn fwd_start(
    state: State<'_, AppState>,
    conn: ConnId,
    spec: Forward,
) -> Result<ForwardStatus, CmdError> {
    fwd_start_op(&state, conn, spec).await
}

#[tauri::command]
pub async fn fwd_stop(
    state: State<'_, AppState>,
    conn: ConnId,
    id: ForwardId,
) -> Result<(), CmdError> {
    fwd_stop_op(&state, conn, id).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn local(bind: &str, port: u16, host: &str, host_port: u16) -> Forward {
        Forward::Local {
            bind: bind.into(),
            port,
            host: host.into(),
            host_port,
        }
    }

    #[test]
    fn valida_especificaciones() {
        // Loopback y por defecto: sin aviso.
        assert_eq!(check_forward(&local("", 8080, "db", 5432)).unwrap(), None);
        assert_eq!(
            check_forward(&local("127.0.0.1", 8080, "db", 5432)).unwrap(),
            None
        );
        assert_eq!(check_forward(&local("::1", 0, "db", 5432)).unwrap(), None);
        assert_eq!(
            check_forward(&Forward::Dynamic {
                bind: "localhost".into(),
                port: 1080
            })
            .unwrap(),
            None
        );
        // Bind público: se permite pero avisa.
        assert!(
            check_forward(&local("0.0.0.0", 8080, "db", 5432))
                .unwrap()
                .unwrap()
                .starts_with("aviso")
        );
        // Rechazos.
        for bad in [
            local("no-es-ip", 8080, "db", 5432),
            local("10.0.0.1 ", 8080, "db", 5432),
            local("", 8080, "", 5432),
            local("", 8080, "a b", 5432),
            local("", 8080, "db", 0),
        ] {
            assert_eq!(
                check_forward(&bad).unwrap_err().kind,
                ErrorKind::Invalid,
                "{bad:?}"
            );
        }
        let remote = Forward::Remote {
            bind: "".into(),
            port: 9000,
            host: "h".into(),
            host_port: 0,
        };
        assert!(check_forward(&remote).is_err());
        assert!(
            check_forward(&Forward::Dynamic {
                bind: "x".into(),
                port: 1
            })
            .is_err()
        );
    }

    #[test]
    fn conexion_desconocida_es_notfound() {
        let st = AppState::default();
        assert_eq!(fwd_list_op(&st, 42).unwrap_err().kind, ErrorKind::Notfound);
    }
}
