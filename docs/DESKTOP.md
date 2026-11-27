# FrikiTerm Desktop (Tauri v2): plan y contrato

Interfaz gráfica tipo Termius/SecureCRT sobre el núcleo Rust. La terminal se pinta con xterm.js; los secretos nunca entran en el webview.

## Stack

Svelte 5 (runes) + Vite + TypeScript, sin kit de UI. `@xterm/xterm`, `@xterm/addon-fit`, `@xterm/addon-webgl` (si se pierde el contexto WebGL, se descarta y se usa el renderizado DOM), `@tauri-apps/api`.

## Estructura

```
apps/desktop/
  package.json  vite.config.ts  svelte.config.js  tsconfig.json  index.html
  src/  App.svelte  lib/{api.ts,types.ts,state.svelte.ts,theme.css}  views/*  components/*
  src-tauri/
    Cargo.toml        # paquete friki-desktop con su PROPIO [workspace] y Cargo.lock
    build.rs          # tauri_build + AppManifest::commands(&[...]) (lista blanca)
    tauri.conf.json  capabilities/main.json  deny.toml  icons/
    src/ main.rs state.rs error.rs dto.rs cmd/{hosts,term,sftp,keys,fwd}.rs
```

- El `Cargo.toml` raíz lleva `exclude = ["apps/desktop/src-tauri"]`: el CI actual (`--workspace`, MSRV) no necesita webkit2gtk. Los `friki-*` son dependencias por ruta (`../../../crates/...`).
- `src-tauri/deny.toml` copia la política raíz y añade las excepciones que pida `cargo deny check`, cada una con motivo y fecha de revisión.

## Contrato Rust ↔ UI

Error de todos los comandos: `Result<T, CmdError>` con `CmdError { kind, message }`, `kind ∈ "auth" | "hostkey" | "network" | "notfound" | "denied" | "invalid" | "unsupported" | "other"` (desde `friki_ssh::Error` y `friki_xfer::ErrorKind`). Identificadores `u64`: `TermId`, `ConnId`, `SftpId`, `PromptId`, `JobId`.

### Eventos globales

`app_subscribe(events: Channel<AppEvent>)` al arrancar (con Channel no hace falta permiso de eventos).

```rust
enum AppEvent {
  Prompt { id: PromptId, prompt: Prompt },
  Transfer { sftp: SftpId, ev: TransferEvent },   // refleja QueueEvent
  Forward { conn: ConnId, status: ForwardStatus },
  ConnClosed { conn: ConnId, reason: Option<String> },
}
enum Prompt {
  HostKeyUnknown { host, port, algorithm, fingerprint },
  HostKeyChanged { host, port, algorithm, fingerprint, previous_fingerprint },
  Password { user, host }, Passphrase { key_path },
  KbdInteractive { host, name, instructions, prompts: Vec<{ text, echo }> },
}
```

`prompt_answer(id, answer: PromptAnswer)`, con `PromptAnswer = HostKey { decision: Reject | AcceptOnce | AcceptAndSave } | Secret { value: Option<Zeroizing<String>>, remember: bool } | Multi { values: Option<Vec<Zeroizing<String>>> }`.

- Rust guarda `PromptId → (tipo esperado, oneshot)`. Los id son aleatorios y de un solo uso, y una respuesta de otro tipo da `invalid`.
- 120 s sin respuesta, o cerrar la ventana, cuenta como rechazo.
- `remember` guarda el secreto con `friki_core::secrets` (`password:{user}@{host}:{port}`). Antes de preguntar se mira el llavero.
- Rust nunca envía un secreto al webview.

### Hosts

- `hosts_list() -> Vec<SessionConfig>`
- `host_save(host, previous_name: Option<String>)`: valida el nombre con `safe_name`, el puerto, la carpeta, y rechaza `protocol != Tn3270`. Al renombrar, borra el fichero anterior.
- `host_delete(name)`
- `hosts_import_ssh_config() -> { added, warnings }`

### Terminal

- `term_open(target: Saved{name} | Quick{spec} | Local, cols, rows, output: Channel<bytes>, events: Channel<TermEvent>) -> TermId`. Vuelve al momento y conecta en segundo plano.
  - La salida va en bytes crudos (ArrayBuffer), agrupados en trozos de unos 64 KB cada ~16 ms.
  - `TermEvent = Opened { conn: Option<ConnId>, warnings, negotiated, forwards } | Failed { error } | Exit { code } | Closed | Lost { reason }`.
- `term_write(id, data: Vec<u8>)`, `term_resize(id, cols, rows)`, `term_close(id)`.

### Conexiones (solo SSH)

- `conn_open(host) -> ConnId`, `conn_close(conn)`.
- El SSH de una terminal se registra como `ConnId` (`Arc<Connection>`), así SFTP y redirecciones la reutilizan. La conexión se cierra cuando se va su último usuario.

### SFTP y transferencias

- `sftp_open(conn) -> SftpId`
- `local_root() -> String` (home)
- `local_pick_root() -> Option<String>`: el diálogo lo abre Rust con `tauri-plugin-dialog`; el webview no tiene permiso de diálogos.
- `fs_list(fs: Local{root} | Remote{sftp}, path) -> Vec<EntryDto>`, `fs_mkdir`, `fs_rename(fs, from, to)`, `fs_remove(fs, path, recursive)`. Lo local pasa siempre por `LocalFs::new(root)`.
- `xfer_enqueue(sftp, local_root, jobs: Vec<{direction, src, dst, recursive}>) -> Vec<JobId>`: una `friki_xfer::Queue` por (sftp, raíz), con `Resume::Auto`.
- `xfer_upload_paths(sftp, paths, remote_dir) -> Vec<JobId>`: para ficheros soltados desde el sistema. Cada ruta debe ser absoluta y existir, y se encola con `LocalFs` en su directorio padre.
- `xfer_cancel(sftp, job)`, `xfer_pause(sftp, paused)`

### Llavero y snippets

- `keys_list() -> Vec<KeyInfo { path, algorithm, comment, fingerprint, encrypted }>`: recorre los `*.pub` de `~/.ssh` y de `config_dir/keys`, solo datos públicos.
- `key_generate(name, comment, passphrase: Option<Zeroizing<String>>) -> KeyInfo` (Ed25519 en `keys_dir`).
- `key_import() -> Option<KeyInfo>`: diálogo en Rust; copia con permisos 0600.
- `key_install(conn, key_path) -> bool`: `key_path` tiene que ser uno de los que devuelve `keys_list`.
- `snippets_list() -> Vec<Snippet { name, command }>`, `snippets_save(Vec<Snippet>)` (`config_dir/snippets.toml`). Enviar un snippet a varias sesiones es un bucle de `term_write` en la UI.

### Metadatos de hosts

- `hosts_meta() -> Map<nombre, HostMeta { lastConnected?: i64 (s Unix), os?: string, connectCount: u32 }>` (`config_dir/desktop-hosts.toml`, atómico, sin secretos). Solo lectura: el backend anota `lastConnected`/`connectCount` al abrir con éxito un host guardado (`term_open`/`conn_open`) y detecta `os` una vez en segundo plano con `uname -s; cat /etc/os-release` por un canal exec de la misma conexión (plazo 5 s, fallos ignorados). `os`: `linux`, id de distro (`debian`, `ubuntu`, `alpine`, `rhel`, `fedora`, `arch`...), `macos`, `freebsd`/`openbsd`/`netbsd`, `windows`. `host_save` (renombrado) y `host_delete` mueven/borran la entrada.

### Ajustes y utilidades

- `settings_get() -> DesktopSettings`, `settings_save(DesktopSettings)` (`config_dir/desktop.toml`, escritura atómica). Campos camelCase: `theme` (system|light|dark), `fontFamily`, `fontSize` (8-32), `cursorStyle` (block|underline|bar), `cursorBlink`, `scrollback` (0-100000), `copyOnSelect`, `rightClick` (paste|menu), `confirmClose`. Campos desconocidos o ausentes: se ignoran / valor por defecto; un fichero inválido da los valores por defecto. `settings_save` rechaza valores fuera de rango (`invalid`).
- `open_url(url)`: abre en el navegador del SO; solo `http://` y `https://` (sin espacios ni caracteres de control). El webview no tiene permiso de opener.

### Redirecciones

- `fwd_list(conn) -> Vec<ForwardStatus { id, spec: Forward, listen, active, error }>`
- `fwd_start(conn, spec) -> ForwardStatus`: valida puertos y bind. Un bind que no sea de loopback lleva un aviso.
- `fwd_stop(conn, id)`
- Las redirecciones guardadas en el host se activan al conectar. Los interruptores solo afectan a la conexión en curso.

## Crate compartido `crates/friki-session`

Se extrae de friki-tui (y de friki-cli) para que TUI y GUI compartan la lógica:

| Hoy | En friki-session |
|---|---|
| `friki-tui/src/app.rs` `UiChannel::ask` | `trait Asker: Send + Sync + Clone { async fn ask(&self, p: Prompt) -> Option<Reply>; }` |
| `app.rs` `UiPrompter` | `Prompter<A: Asker>`, que implementa `AuthPrompter` |
| closure verificador de `app.rs` | `fn verifier<A>(asker, known_hosts)` |
| `app.rs` `open_ssh` | `connect_ssh(asker, &SessionConfig, known_hosts) -> Connection` y `open_ssh` (shell + redirecciones) |
| `app.rs` `open_stream` y `Opened` | `open_stream(asker, &SessionConfig, known_hosts, (cols, rows))`, donde `Opened.conn: Option<Arc<Connection>>` |
| `friki-tui/src/target.rs` (`tag`, `label`, `term_for`, `telnet_options`, `forward_label`) | se mueven aquí y la TUI los reexporta |
| `friki-cli/src/pubkey.rs` (`read_pubkey`, `key_id`, `with_key`, `add_key`) | `friki_session::pubkey` más `install(conn, línea) -> bool`; la CLI queda como envoltorio |

Los textos de los diálogos siguen en la TUI, cuyo `Asker` traduce `Prompt` a `Dialog`. En friki-core solo se añaden funciones: `config::delete_session`, `paths::keys_dir`, `paths::snippets_file`.

## UI

```
App.svelte
 ├ NavRail (Hosts · SFTP · Llavero · Snippets · Redirecciones)
 ├ HostsView: búsqueda, árbol de carpetas > fila de host (doble clic conecta), HostForm en cajón (General | Auth | Terminal | Redirecciones | Serie)
 ├ Workspace: TabBar (+ conexión rápida) > Split (2 niveles, CSS grid) > TerminalPane (xterm + fit + webgl, ResizeObserver → term_resize)
 ├ SftpView: FilePane local | FilePane remoto (arrastrar entre paneles; soltar desde el sistema → xfer_upload_paths), cola de transferencias
 ├ KeychainView · SnippetsView (+ SnippetRunner) · ForwardsView (interruptores por conexión abierta)
 └ PromptModal (cola FIFO; huella en monoespaciada; contraseña + «recordar») · avisos
```

- El estado vive en `lib/state.svelte.ts` (runes). `lib/api.ts` es el único sitio que llama a `invoke`. `lib/types.ts` copia a mano los DTO de Rust.
- El tema es `theme.css` con variables CSS: sigue `prefers-color-scheme` y se puede forzar con `[data-theme]`. El tema de xterm lee las mismas variables.
- Fuera de v1: log de sesión, keymap y TN3270 (devuelve `unsupported`).

## Seguridad

- **CSP:** `default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; font-src 'self'; connect-src ipc: http://ipc.localhost; object-src 'none'; base-uri 'none'; form-action 'none'; frame-src 'none'`.
- **Configuración de Tauri:** `withGlobalTauri: false` (sin `freezePrototype`: xterm.js asigna `toString` en objetos y con el prototipo congelado la UI no arranca), sin URLs remotas y sin devtools en release.
- **Capacidades:** una sola, `capabilities/main.json`, con los comandos de la app (lista blanca de build.rs) y `core:event:allow-listen`/`allow-unlisten` (solo para el drag & drop). Sin `fs`, `shell` ni `dialog`.
- **Validación en Rust de todo lo que llega:** nombres, puertos, bind, que los id existan, que la respuesta coincida con el tipo de prompt, rutas vía `LocalFs`, y que `key_path` esté en `keys_list`.
- **Secretos:** se deserializan directamente a `Zeroizing`.

## Pruebas y CI

- **Rust:** los comandos son envoltorios finos sobre `fn op(&AppState, …)`, así que se prueban sin el runtime de Tauri: validación, registro de prompts (tipo equivocado, doble respuesta, plazo), `keys_list`, redirecciones, y `Target::Local` con `echo hi`.
- **friki-session:** pruebas con un `Asker` falso, y SSH con las variables `FRIKI_TEST_*`.
- **UI:** `npm run check` (svelte-check), `npm run build` y vitest (árbol de carpetas y filtro de búsqueda).
- **CI:** `.github/workflows/desktop.yml` en ubuntu y macOS:
  1. En Linux, `apt install libwebkit2gtk-4.1-dev librsvg2-dev`.
  2. `npm ci && npm run check && npm run build && npm audit --omit=dev --audit-level=high`.
  3. En `src-tauri`: fmt, clippy `-D warnings`, test y `cargo deny check`.
  4. La UI se compila antes que cargo, porque `generate_context!` necesita `frontendDist`.

## Reparto

- **Oleada 1:**
  - T0: andamiaje y contrato (dto, error, state, comandos como stubs `unsupported`, configuración, types.ts, api.ts, esqueleto de App).
  - T1: friki-session.
- **Oleada 2:**
  - T2: backend de hosts, terminal, conexiones y prompts.
  - T3: backend de SFTP, llavero, snippets y redirecciones.
  - T4: UI de shell, hosts y terminal.
  - T5: UI de SFTP, llavero, snippets y redirecciones.
  - T6: CI y documentación.
- **Oleada 3:** integración con `cargo tauri dev` contra el sshd de pruebas.

Solo T0 toca `dto.rs`, `types.ts` y `api.ts`. Un cambio de contrato se pide al coordinador.

## Formulario de hosts (puertos serie, identidades)
- `serial_ports() -> Vec<SerialPortInfo { name, description }>`: lista de `friki_serial::list_ports` (igual que `friki serial list`). `description` es orientativa (USB, Bluetooth, Puerto COM) y puede ir vacía. Bloqueante en un hilo aparte.
- `pick_identity_file() -> Option<String>`: diálogo nativo en Rust (`tauri-plugin-dialog`) que empieza en `~/.ssh`. `None` si se cancela.
- Validación del formulario en `lib/hostForm.ts` (sin IPC; `npm test`). Replica `hosts::validate` y añade: puerto 1-65535, velocidad entero positivo (lista sugerida o personalizada), bits de datos 5-8, parada 1-2, timeout/reintentos enteros y destino de redirecciones. «Guardar» está desactivado hasta que el borrador es válido.
- Pestañas por protocolo: SSH (General, Autenticación, Terminal, Redirecciones), serie (General, Terminal, Serie), telnet/raw/telnet-tls (General, Terminal), shell local (General, Terminal). TN3270 no aparece.
