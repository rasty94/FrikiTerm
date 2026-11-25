//! Terminal y conexiones SSH (stubs, T2).
#![allow(unused_variables)] // stubs

use tauri::State;
use tauri::ipc::{Channel, InvokeResponseBody};

use crate::dto::{ConnId, TermEvent, TermId, TermTarget};
use crate::error::{CmdError, unsupported};
use crate::state::AppState;

/// Vuelve al momento y conecta en segundo plano; `output` lleva bytes crudos.
#[tauri::command]
pub async fn term_open(
    state: State<'_, AppState>,
    target: TermTarget,
    cols: u16,
    rows: u16,
    output: Channel<InvokeResponseBody>,
    events: Channel<TermEvent>,
) -> Result<TermId, CmdError> {
    unsupported("term_open")
}

#[tauri::command]
pub async fn term_write(
    state: State<'_, AppState>,
    id: TermId,
    data: Vec<u8>,
) -> Result<(), CmdError> {
    unsupported("term_write")
}

#[tauri::command]
pub async fn term_resize(
    state: State<'_, AppState>,
    id: TermId,
    cols: u16,
    rows: u16,
) -> Result<(), CmdError> {
    unsupported("term_resize")
}

#[tauri::command]
pub async fn term_close(state: State<'_, AppState>, id: TermId) -> Result<(), CmdError> {
    unsupported("term_close")
}

#[tauri::command]
pub async fn conn_open(state: State<'_, AppState>, host: String) -> Result<ConnId, CmdError> {
    unsupported("conn_open")
}

#[tauri::command]
pub async fn conn_close(state: State<'_, AppState>, conn: ConnId) -> Result<(), CmdError> {
    unsupported("conn_close")
}
