//! Hosts guardados y avisos/respuestas globales. Resto: stubs `unsupported` (T2).
#![allow(unused_variables)] // stubs

use tauri::State;
use tauri::ipc::Channel;

use crate::dto::{AppEvent, ImportReport, PromptAnswer, PromptId};
use crate::error::{CmdError, unsupported};
use crate::state::AppState;
use friki_core::config::SessionConfig;

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
pub async fn hosts_list(state: State<'_, AppState>) -> Result<Vec<SessionConfig>, CmdError> {
    unsupported("hosts_list")
}

#[tauri::command]
pub async fn host_save(
    state: State<'_, AppState>,
    host: SessionConfig,
    previous_name: Option<String>,
) -> Result<(), CmdError> {
    unsupported("host_save")
}

#[tauri::command]
pub async fn host_delete(state: State<'_, AppState>, name: String) -> Result<(), CmdError> {
    unsupported("host_delete")
}

#[tauri::command]
pub async fn hosts_import_ssh_config(state: State<'_, AppState>) -> Result<ImportReport, CmdError> {
    unsupported("hosts_import_ssh_config")
}
