# Cambios de contrato en friki-tui (multiplexor)

## API pública

- `run(session)` se mantiene: una sola sesión; al terminar el remoto la aplicación sale sin
  esperar tecla, con el mismo código que antes y los `friki_ssh::Error` sin envolver.
- `run_app(initial: Option<SessionConfig>, paths: Paths)`: multiplexor completo. Sin sesión
  inicial abre el selector de sesiones. Un panel terminado/fallido espera una tecla para cerrarse.
- `run_with(tui, events, initial, paths, exit_with_last)`: el bucle genérico (backend de
  ratatui + flujo de eventos) que usan `run`, `run_app` y las pruebas.
- `Paths` tiene `known_hosts`, `logs_dir`, `sessions_dir` y `Paths::default_paths()`. Sin cambios
  en esta oleada.
- Eliminados `shell_session`, `connect_ui` y `Connected`. `friki-cli` solo usaba `run`.

## Paneles genéricos (fases 3-4)

- Cada panel trabaja con `friki_core::transport::Stream`; el protocolo sale de
  `session.protocol`: SSH (`Connection::connect` + `open_shell_stream`), Telnet/TelnetTls/Raw
  (`friki_telnet::connect`), Serial (`friki_serial::open`), LocalShell (`friki_pty::spawn` con
  `local_command`). Los diálogos de clave de host y autenticación solo existen en SSH.
- SSH arranca además `session.forwards` con `start_forward`; las activas se listan en la barra
  de estado (`L 127.0.0.1:puerto>host:puerto`, `R ...`, `D ... (SOCKS5)`) y los fallos salen en
  el aviso modal del panel (el shell no se tumba).
- Códigos de salida: `Exit(n)` = n; `Closed` = 0 (o el `Exit` previo); `Lost` y el cierre sin
  estado = `friki_ssh::Error::Network` sin envolver, también en Telnet/serie/PTY. Un fallo al
  abrir un transporte que no es SSH es un `Error::Network` con el texto del error de E/S.
  Con SSH, `ExitStatus` fuera de `i32` ahora da `i32::MAX` (lo decide `friki-ssh`), antes 255.
- Dependencias nuevas: `friki-telnet`, `friki-serial`, `friki-pty`.

## Perfiles y temas

- Emulador: `Terminal::with_profile(filas, columnas, &session.terminal)`; el scrollback sale del
  perfil (la constante `SCROLLBACK` desaparece). `profile_warnings()` y los avisos de conexión
  van juntos en el aviso modal del panel y en el registro de sesión.
- `TERM` anunciado (SSH, TTYPE de Telnet, PTY local): si `session.terminal` es el perfil por
  defecto, manda `session.term`; si el perfil se ha personalizado, manda
  `Terminal::term_name(emulation)` y `session.term` se ignora (`target::term_for`).
- Temas (`theme.rs`): `default` (no toca nada, paleta del terminal), `solarized-dark` y `light`
  sustituyen el primer plano/fondo por defecto y los colores indexados 0-15. Un tema desconocido
  avisa y cae a `default`.

## Configuración y atajos

- `App::new` carga `AppConfig` con `load_app_config` sobre `config.toml` junto a `sessions_dir`
  (con las rutas por defecto es `paths::app_config_file()`; así las pruebas con directorios
  temporales no leen la configuración real). Error = aviso y valores por defecto.
- `keys::parse_chord_local` eliminado: se usa `friki_term::parse_chord`. `Keymap::new` sigue con la
  misma firma y gana `Keymap::warnings()`: prefijo no válido (cae al por defecto), atajos no
  válidos, repetidos tras normalizar (`shift+o` = `O`; gana el primero en el orden de acciones) o
  iguales al prefijo se ignoran con aviso. Los avisos se muestran una vez en el selector (línea
  de error) o, si hay sesión inicial, en el aviso del primer panel.

## Selector de sesiones

- Árbol por carpetas (`session_tree`): izquierda/derecha pliegan y despliegan, Enter sobre una
  carpeta alterna y sobre una sesión la abre. Escribir filtra (lista plana) sobre `carpeta/nombre`,
  host, `usuario@host` y etiqueta del protocolo. Cada sesión indica su protocolo (`[ssh]`,
  `[telnet]`, `[telnets]`, `[raw]`, `[serie]`, `[local]`).

## Pendiente de otras oleadas

- Las acciones `search` y `copy-mode` están reconocidas y muestran "pendiente" en la barra.

## Notas

- Atajos: normalización de teclas (letras en minúscula con `shift` si eran mayúsculas; símbolos
  sin `shift`; `Ctrl+5` equivale a `Ctrl+]`), así que `shift+o` coincide con `O`.
- `split-horizontal` apila los paneles (arriba/abajo, como `"` de tmux); `split-vertical` los
  pone lado a lado (`%`). La división horizontal no gasta una fila de separador: el título del
  panel de abajo hace de línea.
- Las tareas de panel reenvían los eventos del flujo por un canal sin límite al bucle central.
- Pruebas de integración `#[ignore]` (`tests/session.rs`, convención `FRIKI_TEST_*` y
  `FRIKI_TEST_REQUIRED=1`): SSH, SSH + Telnet a la vez (`FRIKI_TEST_TELNET_PORT`) y panel
  LocalShell con `echo hola; exit 4` que termina con código 4 (no necesita servidor).
