//! SFTP, sistema de ficheros y transferencias (stubs, T3).
#![allow(unused_variables)] // stubs

use tauri::State;

use crate::dto::{ConnId, EntryDto, FsRef, JobId, SftpId, XferJob};
use crate::error::{CmdError, unsupported};
use crate::state::AppState;

#[tauri::command]
pub async fn sftp_open(state: State<'_, AppState>, conn: ConnId) -> Result<SftpId, CmdError> {
    unsupported("sftp_open")
}

#[tauri::command]
pub async fn local_root(state: State<'_, AppState>) -> Result<String, CmdError> {
    unsupported("local_root")
}

/// El diálogo lo abre Rust con tauri-plugin-dialog.
#[tauri::command]
pub async fn local_pick_root(state: State<'_, AppState>) -> Result<Option<String>, CmdError> {
    unsupported("local_pick_root")
}

#[tauri::command]
pub async fn fs_list(
    state: State<'_, AppState>,
    fs: FsRef,
    path: String,
) -> Result<Vec<EntryDto>, CmdError> {
    unsupported("fs_list")
}

#[tauri::command]
pub async fn fs_mkdir(state: State<'_, AppState>, fs: FsRef, path: String) -> Result<(), CmdError> {
    unsupported("fs_mkdir")
}

#[tauri::command]
pub async fn fs_rename(
    state: State<'_, AppState>,
    fs: FsRef,
    from: String,
    to: String,
) -> Result<(), CmdError> {
    unsupported("fs_rename")
}

#[tauri::command]
pub async fn fs_remove(
    state: State<'_, AppState>,
    fs: FsRef,
    path: String,
    recursive: bool,
) -> Result<(), CmdError> {
    unsupported("fs_remove")
}

#[tauri::command]
pub async fn xfer_enqueue(
    state: State<'_, AppState>,
    sftp: SftpId,
    local_root: String,
    jobs: Vec<XferJob>,
) -> Result<Vec<JobId>, CmdError> {
    unsupported("xfer_enqueue")
}

#[tauri::command]
pub async fn xfer_upload_paths(
    state: State<'_, AppState>,
    sftp: SftpId,
    paths: Vec<String>,
    remote_dir: String,
) -> Result<Vec<JobId>, CmdError> {
    unsupported("xfer_upload_paths")
}

#[tauri::command]
pub async fn xfer_cancel(
    state: State<'_, AppState>,
    sftp: SftpId,
    job: JobId,
) -> Result<(), CmdError> {
    unsupported("xfer_cancel")
}

#[tauri::command]
pub async fn xfer_pause(
    state: State<'_, AppState>,
    sftp: SftpId,
    paused: bool,
) -> Result<(), CmdError> {
    unsupported("xfer_pause")
}
