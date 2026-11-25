//! Redirecciones de puertos (stubs, T3).
#![allow(unused_variables)] // stubs

use tauri::State;

use crate::dto::{ConnId, ForwardId, ForwardStatus};
use crate::error::{CmdError, unsupported};
use crate::state::AppState;
use friki_core::config::Forward;

#[tauri::command]
pub async fn fwd_list(
    state: State<'_, AppState>,
    conn: ConnId,
) -> Result<Vec<ForwardStatus>, CmdError> {
    unsupported("fwd_list")
}

#[tauri::command]
pub async fn fwd_start(
    state: State<'_, AppState>,
    conn: ConnId,
    spec: Forward,
) -> Result<ForwardStatus, CmdError> {
    unsupported("fwd_start")
}

#[tauri::command]
pub async fn fwd_stop(
    state: State<'_, AppState>,
    conn: ConnId,
    id: ForwardId,
) -> Result<(), CmdError> {
    unsupported("fwd_stop")
}
