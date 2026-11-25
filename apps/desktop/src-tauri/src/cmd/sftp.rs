//! SFTP, sistema de ficheros (local y remoto) y transferencias.

use std::collections::HashMap;
use std::future::Future;
use std::path::{Component, Path, PathBuf};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::sync::{Arc, MutexGuard, PoisonError};

use friki_xfer::local::LocalFs;
use friki_xfer::queue::{Direction as QDir, Job, Queue, QueueEvent, QueueOptions, Resume};
use friki_xfer::{Entry, Kind, RemoteFs, join, safe_name};
use tauri::{AppHandle, State};
use tauri_plugin_dialog::{DialogExt, FilePath};
use tokio::sync::{mpsc, oneshot};

use crate::dto::{
    AppEvent, ConnId, Direction, EntryDto, EntryKind, FsRef, JobId, SftpId, TransferEvent, XferJob,
};
use crate::error::{CmdError, ErrorKind};
use crate::state::{AppState, QueueHandle, SftpHandle};

/// Tope de trabajos por llamada y de longitud de ruta (IPC no confiable).
const MAX_BATCH: usize = 1000;
const MAX_PATH: usize = 4096;

fn queues(state: &AppState) -> MutexGuard<'_, HashMap<(SftpId, String), QueueHandle>> {
    state.queues.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Directorio personal del usuario.
pub(crate) fn home() -> Result<PathBuf, CmdError> {
    directories::BaseDirs::new()
        .map(|d| d.home_dir().to_path_buf())
        .ok_or_else(|| CmdError::new(ErrorKind::Other, "sin directorio personal"))
}

/// Diálogo nativo (lo abre Rust: el webview no tiene `dialog`). `None` = cancelado.
pub(crate) async fn pick(app: &AppHandle, folder: bool) -> Option<PathBuf> {
    let (tx, rx) = oneshot::channel();
    let cb = move |p: Option<FilePath>| {
        let _ = tx.send(p.and_then(|p| p.into_path().ok()));
    };
    let dialog = app.dialog().file();
    if folder {
        dialog.pick_folder(cb);
    } else {
        dialog.pick_file(cb);
    }
    rx.await.ok().flatten()
}

fn check_path(p: &str) -> Result<(), CmdError> {
    if p.is_empty() || p.len() > MAX_PATH || p.contains('\0') {
        return Err(CmdError::invalid("ruta no válida"));
    }
    Ok(())
}

/// Ruta local relativa a su raíz (LocalFs la vuelve a comprobar al ejecutarla).
fn check_rel(p: &str) -> Result<(), CmdError> {
    check_path(p)?;
    if Path::new(p)
        .components()
        .any(|c| !matches!(c, Component::Normal(_)))
    {
        return Err(CmdError::invalid("ruta local fuera de la raíz"));
    }
    Ok(())
}

/// Raíz local válida: absoluta y directorio existente.
fn local_fs(root: &str) -> Result<LocalFs, CmdError> {
    let p = Path::new(root);
    if root.is_empty() || !p.is_absolute() || !p.is_dir() {
        return Err(CmdError::invalid("raíz local no válida"));
    }
    Ok(LocalFs::new(p))
}

fn remote_fs(state: &AppState, sftp: SftpId) -> Result<Arc<dyn RemoteFs>, CmdError> {
    state
        .sftps
        .with(sftp, |h| h.fs.clone())
        .ok_or_else(|| CmdError::new(ErrorKind::Notfound, "sesión SFTP desconocida"))
}

fn fs_for(state: &AppState, fs: &FsRef) -> Result<Arc<dyn RemoteFs>, CmdError> {
    match fs {
        FsRef::Local { root } => Ok(Arc::new(local_fs(root)?)),
        FsRef::Remote { sftp } => remote_fs(state, *sftp),
    }
}

fn entry_dto(e: Entry) -> EntryDto {
    EntryDto {
        name: e.name,
        path: e.path,
        kind: match e.kind {
            Kind::File => EntryKind::File,
            Kind::Dir => EntryKind::Dir,
            Kind::Symlink => EntryKind::Symlink,
            Kind::Other => EntryKind::Other,
        },
        size: e.size,
        mtime: e.mtime,
        mode: e.mode,
    }
}

// ------------------------------------------------------------------ sesión SFTP

#[tauri::command]
pub async fn sftp_open(state: State<'_, AppState>, conn: ConnId) -> Result<SftpId, CmdError> {
    let c = state.conn(conn)?;
    let fs: Arc<dyn RemoteFs> = Arc::from(friki_xfer::sftp::open(&c).await?);
    Ok(state.sftps.insert(SftpHandle {
        fs,
        paused: AtomicBool::new(false),
    }))
}

#[tauri::command]
pub async fn local_root(_state: State<'_, AppState>) -> Result<String, CmdError> {
    Ok(home()?.to_string_lossy().into_owned())
}

/// El diálogo lo abre Rust con tauri-plugin-dialog.
#[tauri::command]
pub async fn local_pick_root(app: AppHandle) -> Result<Option<String>, CmdError> {
    Ok(pick(&app, true)
        .await
        .map(|p| p.to_string_lossy().into_owned()))
}

// ------------------------------------------------------------------ sistema de ficheros

pub async fn fs_list_op(
    state: &AppState,
    fs: &FsRef,
    path: &str,
) -> Result<Vec<EntryDto>, CmdError> {
    // "" = raíz (o cwd en SFTP): se pide como ".".
    let path = if path.is_empty() { "." } else { path };
    check_path(path)?;
    let r = fs_for(state, fs)?;
    Ok(r.list(path).await?.into_iter().map(entry_dto).collect())
}

pub async fn fs_mkdir_op(state: &AppState, fs: &FsRef, path: &str) -> Result<(), CmdError> {
    check_path(path)?;
    Ok(fs_for(state, fs)?.mkdir(path).await?)
}

pub async fn fs_rename_op(
    state: &AppState,
    fs: &FsRef,
    from: &str,
    to: &str,
) -> Result<(), CmdError> {
    check_path(from)?;
    check_path(to)?;
    Ok(fs_for(state, fs)?.rename(from, to).await?)
}

/// Borra `path`; con `recursive`, vacía primero los directorios. Los enlaces no se siguen.
pub async fn fs_remove_op(
    state: &AppState,
    fs: &FsRef,
    path: &str,
    recursive: bool,
) -> Result<(), CmdError> {
    check_path(path)?;
    let r = fs_for(state, fs)?;
    let st = r
        .stat(path)
        .await?
        .ok_or_else(|| CmdError::new(ErrorKind::Notfound, "no existe"))?;
    match (st.kind, recursive) {
        (Kind::Dir, true) => remove_tree(r.as_ref(), path).await?,
        (Kind::Dir, false) => r.remove_dir(path).await?,
        _ => r.remove_file(path).await?,
    }
    Ok(())
}

fn remove_tree<'a>(
    fs: &'a dyn RemoteFs,
    path: &'a str,
) -> Pin<Box<dyn Future<Output = friki_xfer::Result<()>> + Send + 'a>> {
    Box::pin(async move {
        for e in fs.list(path).await? {
            if e.kind == Kind::Dir {
                remove_tree(fs, &e.path).await?;
            } else {
                fs.remove_file(&e.path).await?;
            }
        }
        fs.remove_dir(path).await
    })
}

#[tauri::command]
pub async fn fs_list(
    state: State<'_, AppState>,
    fs: FsRef,
    path: String,
) -> Result<Vec<EntryDto>, CmdError> {
    fs_list_op(&state, &fs, &path).await
}

#[tauri::command]
pub async fn fs_mkdir(state: State<'_, AppState>, fs: FsRef, path: String) -> Result<(), CmdError> {
    fs_mkdir_op(&state, &fs, &path).await
}

#[tauri::command]
pub async fn fs_rename(
    state: State<'_, AppState>,
    fs: FsRef,
    from: String,
    to: String,
) -> Result<(), CmdError> {
    fs_rename_op(&state, &fs, &from, &to).await
}

#[tauri::command]
pub async fn fs_remove(
    state: State<'_, AppState>,
    fs: FsRef,
    path: String,
    recursive: bool,
) -> Result<(), CmdError> {
    fs_remove_op(&state, &fs, &path, recursive).await
}

// ------------------------------------------------------------------ transferencias

/// Id de trabajo global: la cola (`seq`) en lo alto y su id local abajo.
fn gid(seq: u64, local: JobId) -> JobId {
    (seq << 32) | (local & 0xffff_ffff)
}

fn map_event(seq: u64, ev: QueueEvent) -> TransferEvent {
    match ev {
        QueueEvent::Started { id, file, total } => TransferEvent::Started {
            id: gid(seq, id),
            file,
            total,
        },
        QueueEvent::Progress { id, file, done } => TransferEvent::Progress {
            id: gid(seq, id),
            file,
            done,
        },
        QueueEvent::Retrying { id, attempt, error } => TransferEvent::Retrying {
            id: gid(seq, id),
            attempt,
            error,
        },
        QueueEvent::Finished { id, bytes } => TransferEvent::Finished {
            id: gid(seq, id),
            bytes,
        },
        QueueEvent::Failed { id, error, kind } => TransferEvent::Failed {
            id: gid(seq, id),
            error,
            error_kind: match kind {
                friki_xfer::ErrorKind::Network => ErrorKind::Network,
                friki_xfer::ErrorKind::NotFound => ErrorKind::Notfound,
                friki_xfer::ErrorKind::PermissionDenied => ErrorKind::Denied,
                friki_xfer::ErrorKind::Protocol
                | friki_xfer::ErrorKind::Cancelled
                | friki_xfer::ErrorKind::Other => ErrorKind::Other,
            },
        },
        QueueEvent::Cancelled { id } => TransferEvent::Cancelled { id: gid(seq, id) },
    }
}

async fn forward_events(
    state: AppState,
    sftp: SftpId,
    seq: u64,
    mut rx: mpsc::UnboundedReceiver<QueueEvent>,
) {
    while let Some(ev) = rx.recv().await {
        state.emit(AppEvent::Transfer {
            sftp,
            ev: map_event(seq, ev),
        });
    }
}

/// Crea la cola de (sftp, raíz local) si no existe. Requiere runtime de tokio.
fn ensure_queue(state: &AppState, sftp: SftpId, root: &str) -> Result<(), CmdError> {
    let mut qs = queues(state);
    let key = (sftp, root.to_string());
    if qs.contains_key(&key) {
        return Ok(());
    }
    let handle = state
        .sftps
        .with(sftp, |h| (h.fs.clone(), h.paused.load(Relaxed)))
        .ok_or_else(|| CmdError::new(ErrorKind::Notfound, "sesión SFTP desconocida"))?;
    let local: Arc<dyn RemoteFs> = Arc::new(local_fs(root)?);
    // Las colas nunca se quitan: el número de colas + 1 es un `seq` único.
    let seq = qs.len() as u64 + 1;
    let (tx, rx) = mpsc::unbounded_channel();
    let queue = Queue::new(local, handle.0, QueueOptions::default(), tx);
    queue.pause(handle.1);
    tokio::spawn(forward_events(state.clone(), sftp, seq, rx));
    qs.insert(key, QueueHandle { seq, queue });
    Ok(())
}

fn to_job(j: XferJob) -> Job {
    Job {
        direction: match j.direction {
            Direction::Upload => QDir::Upload,
            Direction::Download => QDir::Download,
        },
        src: j.src,
        dst: j.dst,
        recursive: j.recursive,
        // ponytail: reanudación siempre automática; sin opción en el contrato.
        resume: Resume::Auto,
        preserve_mtime: true,
    }
}

fn push_jobs(
    state: &AppState,
    sftp: SftpId,
    root: &str,
    jobs: Vec<Job>,
) -> Result<Vec<JobId>, CmdError> {
    ensure_queue(state, sftp, root)?;
    let qs = queues(state);
    let h = qs
        .get(&(sftp, root.to_string()))
        .ok_or_else(|| CmdError::new(ErrorKind::Other, "cola perdida"))?;
    Ok(jobs
        .into_iter()
        .map(|j| gid(h.seq, h.queue.push(j)))
        .collect())
}

pub fn xfer_enqueue_op(
    state: &AppState,
    sftp: SftpId,
    local_root: &str,
    jobs: Vec<XferJob>,
) -> Result<Vec<JobId>, CmdError> {
    local_fs(local_root)?;
    if jobs.is_empty() || jobs.len() > MAX_BATCH {
        return Err(CmdError::invalid("número de trabajos no válido"));
    }
    for j in &jobs {
        let (local, remote) = match j.direction {
            Direction::Upload => (&j.src, &j.dst),
            Direction::Download => (&j.dst, &j.src),
        };
        check_rel(local)?;
        check_path(remote)?;
    }
    let jobs = jobs.into_iter().map(to_job).collect();
    push_jobs(state, sftp, local_root, jobs)
}

/// Sube rutas absolutas existentes. Cada una usa como raíz su carpeta padre.
pub fn xfer_upload_paths_op(
    state: &AppState,
    sftp: SftpId,
    paths: &[String],
    remote_dir: &str,
) -> Result<Vec<JobId>, CmdError> {
    if paths.is_empty() || paths.len() > MAX_BATCH {
        return Err(CmdError::invalid("número de rutas no válido"));
    }
    check_path(remote_dir)?;
    let mut out = Vec::new();
    for p in paths {
        let abs = Path::new(p);
        let (Some(parent), Some(name)) = (abs.parent(), abs.file_name()) else {
            return Err(CmdError::invalid("ruta no válida"));
        };
        let name = name
            .to_str()
            .filter(|n| safe_name(n))
            .ok_or_else(|| CmdError::invalid("nombre no válido"))?;
        if !abs.is_absolute() || !abs.exists() {
            return Err(CmdError::invalid("la ruta no existe o no es absoluta"));
        }
        let root = parent
            .to_str()
            .ok_or_else(|| CmdError::invalid("ruta no UTF-8"))?;
        let job = Job {
            direction: QDir::Upload,
            src: name.to_string(),
            dst: join(remote_dir, name),
            recursive: abs.is_dir(),
            resume: Resume::Auto,
            preserve_mtime: true,
        };
        out.extend(push_jobs(state, sftp, root, vec![job])?);
    }
    Ok(out)
}

pub fn xfer_cancel_op(state: &AppState, sftp: SftpId, job: JobId) -> Result<(), CmdError> {
    remote_fs(state, sftp)?;
    let (seq, local) = (job >> 32, job & 0xffff_ffff);
    if let Some((_, h)) = queues(state)
        .iter()
        .find(|((s, _), h)| *s == sftp && h.seq == seq)
    {
        h.queue.cancel(local);
    }
    Ok(())
}

pub fn xfer_pause_op(state: &AppState, sftp: SftpId, paused: bool) -> Result<(), CmdError> {
    remote_fs(state, sftp)?;
    state.sftps.with(sftp, |h| h.paused.store(paused, Relaxed));
    for ((s, _), h) in queues(state).iter() {
        if *s == sftp {
            h.queue.pause(paused);
        }
    }
    Ok(())
}

#[tauri::command]
pub async fn xfer_enqueue(
    state: State<'_, AppState>,
    sftp: SftpId,
    local_root: String,
    jobs: Vec<XferJob>,
) -> Result<Vec<JobId>, CmdError> {
    xfer_enqueue_op(&state, sftp, &local_root, jobs)
}

#[tauri::command]
pub async fn xfer_upload_paths(
    state: State<'_, AppState>,
    sftp: SftpId,
    paths: Vec<String>,
    remote_dir: String,
) -> Result<Vec<JobId>, CmdError> {
    xfer_upload_paths_op(&state, sftp, &paths, &remote_dir)
}

#[tauri::command]
pub async fn xfer_cancel(
    state: State<'_, AppState>,
    sftp: SftpId,
    job: JobId,
) -> Result<(), CmdError> {
    xfer_cancel_op(&state, sftp, job)
}

#[tauri::command]
pub async fn xfer_pause(
    state: State<'_, AppState>,
    sftp: SftpId,
    paused: bool,
) -> Result<(), CmdError> {
    xfer_pause_op(&state, sftp, paused)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    /// Directorio temporal único; se borra al salir del test.
    struct Tmp(PathBuf);

    impl Tmp {
        fn new(tag: &str) -> Self {
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let p = std::env::temp_dir()
                .join(format!("friki-sftp-{tag}-{}-{nanos}", std::process::id()));
            std::fs::create_dir_all(&p).unwrap();
            Tmp(p)
        }
        fn s(&self) -> String {
            self.0.to_string_lossy().into_owned()
        }
    }

    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn local(root: &Tmp) -> FsRef {
        FsRef::Local { root: root.s() }
    }

    #[tokio::test]
    async fn fs_crud_en_directorio_temporal() {
        let t = Tmp::new("crud");
        let st = AppState::default();
        let fs = local(&t);
        fs_mkdir_op(&st, &fs, "a").await.unwrap();
        fs_mkdir_op(&st, &fs, "a/b").await.unwrap();
        std::fs::write(t.0.join("a/b/f.txt"), b"hola").unwrap();
        let names: Vec<_> = fs_list_op(&st, &fs, "a")
            .await
            .unwrap()
            .into_iter()
            .map(|e| e.name)
            .collect();
        assert_eq!(names, ["b"]);
        fs_rename_op(&st, &fs, "a/b/f.txt", "a/g.txt")
            .await
            .unwrap();
        assert!(t.0.join("a/g.txt").is_file());
        // No vacío y sin recursivo: falla.
        assert!(fs_remove_op(&st, &fs, "a", false).await.is_err());
        fs_remove_op(&st, &fs, "a", true).await.unwrap();
        assert!(!t.0.join("a").exists());
    }

    #[tokio::test]
    async fn traversal_y_absolutas_se_rechazan() {
        let t = Tmp::new("trav");
        let st = AppState::default();
        let fs = local(&t);
        for bad in ["../x", "a/../../x", "/etc", "a/..", "\0"] {
            let e = fs_mkdir_op(&st, &fs, bad).await.unwrap_err();
            assert!(
                matches!(e.kind, ErrorKind::Denied | ErrorKind::Invalid),
                "{bad:?}: {e:?}"
            );
        }
        let e = fs_list_op(&st, &FsRef::Local { root: "rel".into() }, "")
            .await
            .unwrap_err();
        assert_eq!(e.kind, ErrorKind::Invalid);
        let e = fs_remove_op(&st, &fs, "../etc", true).await.unwrap_err();
        assert!(matches!(e.kind, ErrorKind::Denied | ErrorKind::Invalid));
        assert!(
            fs_list_op(&st, &FsRef::Remote { sftp: 99 }, "/")
                .await
                .is_err()
        );
    }

    #[test]
    fn enqueue_valida_antes_de_encolar() {
        let t = Tmp::new("enq");
        let st = AppState::default();
        let job = |src: &str| XferJob {
            direction: Direction::Upload,
            src: src.into(),
            dst: "x".into(),
            recursive: false,
        };
        // Sin runtime: la validación falla antes de tocar tokio.
        let e = xfer_enqueue_op(&st, 1, &t.s(), vec![job("../fuera")]).unwrap_err();
        assert_eq!(e.kind, ErrorKind::Invalid);
        let e = xfer_enqueue_op(&st, 1, "relativa", vec![job("a")]).unwrap_err();
        assert_eq!(e.kind, ErrorKind::Invalid);
        let e = xfer_enqueue_op(&st, 1, &t.s(), vec![]).unwrap_err();
        assert_eq!(e.kind, ErrorKind::Invalid);
        let e = xfer_upload_paths_op(&st, 1, &["relativa".into()], "/").unwrap_err();
        assert_eq!(e.kind, ErrorKind::Invalid);
    }

    #[test]
    fn job_ids_codifican_la_cola() {
        assert_eq!(gid(3, 7), (3 << 32) | 7);
        assert_eq!(gid(3, 7) >> 32, 3);
        assert!(gid(1 << 20, 1) < (1u64 << 53));
    }

    /// Cola de extremo a extremo con LocalFs como «remoto»: sube un fichero y comprueba el destino.
    #[tokio::test]
    async fn sube_fichero_por_la_cola() {
        let src = Tmp::new("qsrc");
        let dst = Tmp::new("qdst");
        std::fs::write(src.0.join("f.txt"), b"contenido").unwrap();
        let st = AppState::default();
        let remote = SftpHandle {
            fs: Arc::new(LocalFs::new(&dst.0)),
            paused: AtomicBool::new(false),
        };
        let sftp = st.sftps.insert(remote);
        let ids = xfer_enqueue_op(
            &st,
            sftp,
            &src.s(),
            vec![XferJob {
                direction: Direction::Upload,
                src: "f.txt".into(),
                dst: "g.txt".into(),
                recursive: false,
            }],
        )
        .unwrap();
        assert_eq!(ids.len(), 1);
        assert_eq!(ids[0] >> 32, 1);
        let target = dst.0.join("g.txt");
        for _ in 0..200 {
            if target.is_file() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        assert_eq!(std::fs::read(&target).unwrap(), b"contenido");
        xfer_cancel_op(&st, sftp, ids[0]).unwrap();
        xfer_pause_op(&st, sftp, true).unwrap();
    }
}
