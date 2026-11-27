// En release no abre consola en Windows.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
// Andamiaje: los registros y marcadores los usan las tareas de la oleada 2.
#![allow(dead_code)]

mod cmd;
mod dto;
mod error;
#[cfg(test)]
mod integration;
mod state;

use cmd::{fwd, hosts, keys, settings, sftp, term};

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(state::AppState::default())
        // Misma lista que build.rs (lista blanca) y capabilities/main.json.
        .invoke_handler(tauri::generate_handler![
            hosts::app_subscribe,
            hosts::prompt_answer,
            hosts::hosts_list,
            hosts::host_save,
            hosts::host_delete,
            hosts::hosts_import_ssh_config,
            term::term_open,
            term::term_write,
            term::term_resize,
            term::term_close,
            term::conn_open,
            term::conn_close,
            sftp::sftp_open,
            sftp::local_root,
            sftp::local_pick_root,
            sftp::fs_list,
            sftp::fs_mkdir,
            sftp::fs_rename,
            sftp::fs_remove,
            sftp::xfer_enqueue,
            sftp::xfer_upload_paths,
            sftp::xfer_cancel,
            sftp::xfer_pause,
            keys::keys_list,
            keys::key_generate,
            keys::key_import,
            keys::key_install,
            keys::snippets_list,
            keys::snippets_save,
            fwd::fwd_list,
            fwd::fwd_start,
            fwd::fwd_stop,
            settings::settings_get,
            settings::settings_save,
            settings::open_url,
        ])
        .run(tauri::generate_context!())
        .expect("error al arrancar FrikiTerm");
}
