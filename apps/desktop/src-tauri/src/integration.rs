//! Pruebas de integración contra el sshd de docker (`eval "$(scripts/test-sshd.sh up)"`).
//! Sin `FRIKI_TEST_*` en el entorno se saltan sin fallar.

use std::sync::Once;
use std::time::Duration;

use friki_core::config::{AuthMethod, Forward, SessionConfig};
use serde_json::Value;
use tauri::ipc::{Channel, InvokeResponseBody};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc;

use crate::cmd::{fwd, hosts, keys, sftp, term};
use crate::dto::{AppEvent, Direction, FsRef, TermEvent, TermTarget, XferJob};
use crate::state::AppState;

struct Env {
    host: String,
    port: u16,
    user: String,
    key: String,
}

fn env() -> Option<Env> {
    static HOME: Once = Once::new();
    let e = Env {
        host: std::env::var("FRIKI_TEST_HOST").ok()?,
        port: std::env::var("FRIKI_TEST_PORT").ok()?.parse().ok()?,
        user: std::env::var("FRIKI_TEST_USER").ok()?,
        key: std::env::var("FRIKI_TEST_KEY").ok()?,
    };
    // Config aislada (known_hosts, llavero de claves) y nunca la del usuario.
    HOME.call_once(|| {
        let dir = std::env::temp_dir().join(format!("friki-desk-it-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        if let Ok(kh) = std::env::var("FRIKI_TEST_KNOWN_HOSTS") {
            let _ = std::fs::copy(kh, dir.join("known_hosts"));
        }
        // SAFETY: antes de lanzar ninguna tarea que lea el entorno; solo escribe este Once.
        unsafe { std::env::set_var("FRIKI_HOME", &dir) };
    });
    Some(e)
}

fn cfg(e: &Env) -> SessionConfig {
    SessionConfig {
        name: "it".into(),
        host: e.host.clone(),
        port: e.port,
        user: Some(e.user.clone()),
        auth: vec![AuthMethod::PublicKey],
        identity_files: vec![e.key.clone().into()],
        ..Default::default()
    }
}

/// UI falsa: contesta «aceptar una vez» a la clave de host y reenvía el resto de eventos.
fn fake_ui(state: &AppState) -> mpsc::UnboundedReceiver<Value> {
    let (tx, mut rx) = mpsc::unbounded_channel::<InvokeResponseBody>();
    state.subscribe(Channel::<AppEvent>::new(move |b| {
        let _ = tx.send(b);
        Ok(())
    }));
    let (out_tx, out_rx) = mpsc::unbounded_channel();
    let st = state.clone();
    tokio::spawn(async move {
        while let Some(InvokeResponseBody::Json(j)) = rx.recv().await {
            let v: Value = serde_json::from_str(&j).unwrap();
            if v["type"] == "prompt" {
                let ans = serde_json::json!({"kind":"hostKey","decision":"acceptOnce"});
                let _ = st.prompts.answer(
                    v["id"].as_u64().unwrap(),
                    serde_json::from_value(ans).unwrap(),
                );
            } else {
                let _ = out_tx.send(v);
            }
        }
    });
    out_rx
}

async fn conn(state: &AppState, e: &Env) -> u64 {
    let dir = tempfile::tempdir().unwrap();
    hosts::save_in(dir.path(), cfg(e), None).unwrap();
    term::open_conn(state, dir.path(), "it").await.unwrap()
}

async fn recv_until(rx: &mut mpsc::UnboundedReceiver<InvokeResponseBody>, needle: &str) -> String {
    let mut got = Vec::new();
    tokio::time::timeout(Duration::from_secs(20), async {
        while let Some(InvokeResponseBody::Raw(b)) = rx.recv().await {
            got.extend_from_slice(&b);
            if String::from_utf8_lossy(&got).contains(needle) {
                break;
            }
        }
    })
    .await
    .expect("tiempo agotado esperando salida de la terminal");
    String::from_utf8_lossy(&got).into_owned()
}

#[tokio::test]
async fn terminal_ssh_rapida() {
    let Some(e) = env() else { return };
    let state = AppState::default();
    let _ui = fake_ui(&state);
    let (otx, mut orx) = mpsc::unbounded_channel::<InvokeResponseBody>();
    let output = Channel::<InvokeResponseBody>::new(move |b| {
        let _ = otx.send(b);
        Ok(())
    });
    let (etx, mut erx) = mpsc::unbounded_channel::<InvokeResponseBody>();
    let events = Channel::<TermEvent>::new(move |b| {
        let _ = etx.send(b);
        Ok(())
    });
    let dir = tempfile::tempdir().unwrap();
    let id = term::open_term(
        &state,
        dir.path(),
        TermTarget::Quick {
            spec: Box::new(cfg(&e)),
        },
        (80, 24),
        output,
        events,
    )
    .unwrap();
    let Some(InvokeResponseBody::Json(j)) = erx.recv().await else {
        panic!("sin evento opened")
    };
    assert!(j.contains(r#""type":"opened""#), "{j}");
    term::write_term(&state, id, b"echo hola$((1+1))\n".to_vec()).unwrap();
    let out = recv_until(&mut orx, "hola2").await;
    assert!(out.contains("hola2"));
    state_resize(&state, id);
    term::close_term(&state, id);
    let Some(InvokeResponseBody::Json(j)) = erx.recv().await else {
        panic!("sin evento final")
    };
    assert!(j.contains(r#""type":"closed""#), "{j}");
}

/// Cerrar una terminal cierra su conexión, salvo que SFTP la siga usando (entonces vive hasta `conn_close`).
#[tokio::test]
async fn terminal_cierra_su_conexion_salvo_sftp() {
    let Some(e) = env() else { return };
    let state = AppState::default();
    let _ui = fake_ui(&state);
    let (otx, _orx) = mpsc::unbounded_channel::<InvokeResponseBody>();
    let output = Channel::<InvokeResponseBody>::new(move |b| {
        let _ = otx.send(b);
        Ok(())
    });
    let (etx, mut erx) = mpsc::unbounded_channel::<InvokeResponseBody>();
    let events = Channel::<TermEvent>::new(move |b| {
        let _ = etx.send(b);
        Ok(())
    });
    let dir = tempfile::tempdir().unwrap();
    let id = term::open_term(
        &state,
        dir.path(),
        TermTarget::Quick {
            spec: Box::new(cfg(&e)),
        },
        (80, 24),
        output,
        events,
    )
    .unwrap();
    let Some(InvokeResponseBody::Json(j)) = erx.recv().await else {
        panic!("sin evento opened")
    };
    let conn_id: u64 = serde_json::from_str::<Value>(&j).unwrap()["conn"]
        .as_u64()
        .expect("la terminal SSH debe devolver su conexión");
    assert!(state.conns.contains(conn_id));

    // SFTP sobre la misma conexión: al cerrar la terminal la conexión sigue viva.
    let sftp = crate::cmd::sftp::sftp_open_op(&state, conn_id)
        .await
        .unwrap();
    term::close_term(&state, id);
    let Some(InvokeResponseBody::Json(j)) = erx.recv().await else {
        panic!("sin evento final")
    };
    assert!(j.contains(r#""type":"closed""#), "{j}");
    assert!(state.conns.contains(conn_id));
    assert!(state.sftps.contains(sftp));

    // Cerrar la conexión se lleva también su SFTP.
    term::close_conn(&state, conn_id, None).await;
    assert!(!state.conns.contains(conn_id));
    assert!(!state.sftps.contains(sftp));
}

fn state_resize(state: &AppState, id: u64) {
    // `term_resize` valida y envía `TermCmd::Resize`; aquí basta el canal directo.
    state
        .terms
        .with(id, |h| h.tx.send(crate::state::TermCmd::Resize(100, 30)))
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn conexion_sftp_cola_llavero_y_redirecciones() {
    let Some(e) = env() else { return };
    let state = AppState::default();
    let mut ui = fake_ui(&state);
    let c = conn(&state, &e).await;

    // SFTP: listar y subir/bajar por la cola.
    let s = sftp::sftp_open_op(&state, c).await.unwrap();
    let root = sftp::fs_list_op(&state, &FsRef::Remote { sftp: s }, "/")
        .await
        .unwrap();
    assert!(!root.is_empty());
    let local = tempfile::tempdir().unwrap();
    std::fs::write(local.path().join("up.txt"), b"datos de prueba").unwrap();
    let name = format!("friki-it-{}.txt", std::process::id());
    let root_s = local.path().to_string_lossy().into_owned();
    for (dir, src, dst) in [
        (Direction::Upload, "up.txt", name.as_str()),
        (Direction::Download, name.as_str(), "down.txt"),
    ] {
        sftp::xfer_enqueue_op(
            &state,
            s,
            &root_s,
            vec![XferJob {
                direction: dir,
                src: src.into(),
                dst: dst.into(),
                recursive: false,
            }],
        )
        .unwrap();
        tokio::time::timeout(Duration::from_secs(30), async {
            while let Some(v) = ui.recv().await {
                if v["type"] == "transfer" {
                    match v["ev"]["type"].as_str() {
                        Some("finished") => break,
                        Some("failed") => panic!("transferencia fallida: {v}"),
                        _ => {}
                    }
                }
            }
        })
        .await
        .expect("la transferencia no terminó");
    }
    assert_eq!(
        std::fs::read(local.path().join("down.txt")).unwrap(),
        b"datos de prueba"
    );
    sftp::fs_remove_op(&state, &FsRef::Remote { sftp: s }, &name, false)
        .await
        .unwrap();

    // Llavero: generar e instalar (la segunda vez ya estaba).
    let info = keys::key_generate_in(&friki_core::paths::keys_dir(), "it_key", "it", None).unwrap();
    let pubk = info.path;
    assert!(keys::key_install_op(&state, c, &pubk).await.unwrap());
    assert!(!keys::key_install_op(&state, c, &pubk).await.unwrap());

    // Redirección -L al propio sshd: se conecta por ella y llega su banner.
    let spec = Forward::Local {
        bind: "127.0.0.1".into(),
        port: 0,
        host: "127.0.0.1".into(),
        host_port: e.port,
    };
    let st = fwd::fwd_start_op(&state, c, spec).await.unwrap();
    assert!(st.active);
    assert_eq!(fwd::fwd_list_op(&state, c).unwrap().len(), 1);
    let mut tcp = tokio::net::TcpStream::connect(&st.listen).await.unwrap();
    let mut banner = [0u8; 4];
    tokio::time::timeout(Duration::from_secs(10), tcp.read_exact(&mut banner))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(&banner, b"SSH-");
    let _ = tcp.shutdown().await;
    fwd::fwd_stop_op(&state, c, st.id).await.unwrap();
    assert!(fwd::fwd_list_op(&state, c).unwrap().is_empty());

    // Cerrar la conexión avisa a la UI y limpia las redirecciones.
    term::close_conn(&state, c, None).await;
    let closed = tokio::time::timeout(Duration::from_secs(5), async {
        while let Some(v) = ui.recv().await {
            if v["type"] == "connClosed" {
                return v;
            }
        }
        Value::Null
    })
    .await
    .unwrap();
    assert_eq!(closed["conn"], c);
}

/// Redirecciones de la sesión (term_open las arranca): deben verse en `fwd_list`.
#[tokio::test]
async fn redirecciones_de_la_sesion_se_registran() {
    let Some(e) = env() else { return };
    let state = AppState::default();
    let _ui = fake_ui(&state);
    let mut c = cfg(&e);
    c.forwards = vec![Forward::Local {
        bind: "127.0.0.1".into(),
        port: 0,
        host: "127.0.0.1".into(),
        host_port: e.port,
    }];
    let dir = tempfile::tempdir().unwrap();
    hosts::save_in(dir.path(), c, None).unwrap();
    let id = term::open_conn(&state, dir.path(), "it").await.unwrap();
    let l = fwd::fwd_list_op(&state, id).unwrap();
    assert_eq!(l.len(), 1);
    assert!(l[0].active);
    term::close_conn(&state, id, None).await;
    assert!(state.fwds.lock().unwrap().get(&id).is_none());
}
