// Lista blanca de comandos: debe coincidir con generate_handler! de main.rs y con capabilities/main.json.
const COMMANDS: &[&str] = &[
    "app_subscribe",
    "prompt_answer",
    "hosts_list",
    "host_save",
    "host_delete",
    "hosts_import_ssh_config",
    "term_open",
    "term_write",
    "term_resize",
    "term_close",
    "conn_open",
    "conn_close",
    "sftp_open",
    "local_root",
    "local_pick_root",
    "fs_list",
    "fs_mkdir",
    "fs_rename",
    "fs_remove",
    "xfer_enqueue",
    "xfer_upload_paths",
    "xfer_cancel",
    "xfer_pause",
    "keys_list",
    "key_generate",
    "key_import",
    "key_install",
    "snippets_list",
    "snippets_save",
    "fwd_list",
    "fwd_start",
    "fwd_stop",
    "settings_get",
    "settings_save",
    "open_url",
    "serial_ports",
    "pick_identity_file",
];

fn main() {
    tauri_build::try_build(
        tauri_build::Attributes::new()
            .app_manifest(tauri_build::AppManifest::new().commands(COMMANDS)),
    )
    .expect("tauri-build");
}
