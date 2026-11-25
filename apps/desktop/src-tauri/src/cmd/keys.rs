//! Llavero y snippets (stubs, T3).
#![allow(unused_variables)] // stubs

use tauri::State;
use zeroize::Zeroizing;

use crate::dto::{ConnId, KeyInfo, Snippet};
use crate::error::{CmdError, unsupported};
use crate::state::AppState;

#[tauri::command]
pub async fn keys_list(state: State<'_, AppState>) -> Result<Vec<KeyInfo>, CmdError> {
    unsupported("keys_list")
}

#[tauri::command]
pub async fn key_generate(
    state: State<'_, AppState>,
    name: String,
    comment: String,
    passphrase: Option<Zeroizing<String>>,
) -> Result<KeyInfo, CmdError> {
    unsupported("key_generate")
}

/// Diálogo en Rust; copia con permisos 0600.
#[tauri::command]
pub async fn key_import(state: State<'_, AppState>) -> Result<Option<KeyInfo>, CmdError> {
    unsupported("key_import")
}

/// `key_path` tiene que ser uno de los que devuelve `keys_list`.
#[tauri::command]
pub async fn key_install(
    state: State<'_, AppState>,
    conn: ConnId,
    key_path: String,
) -> Result<bool, CmdError> {
    unsupported("key_install")
}

#[tauri::command]
pub async fn snippets_list(state: State<'_, AppState>) -> Result<Vec<Snippet>, CmdError> {
    unsupported("snippets_list")
}

#[tauri::command]
pub async fn snippets_save(
    state: State<'_, AppState>,
    snippets: Vec<Snippet>,
) -> Result<(), CmdError> {
    unsupported("snippets_save")
}
