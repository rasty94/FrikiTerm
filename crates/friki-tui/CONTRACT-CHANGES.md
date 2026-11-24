# Cambios de contrato en friki-tui (multiplexor)

## API pública

- `run(session)` se mantiene: una sola sesión; al terminar el remoto la aplicación sale sin
  esperar tecla, con el mismo código que antes y los `friki_ssh::Error` sin envolver.
- Nuevo `run_app(initial: Option<SessionConfig>, paths: Paths)`: multiplexor completo. Sin sesión
  inicial abre el selector de sesiones. Un panel terminado/fallido espera una tecla para cerrarse.
- Nuevo `run_with(tui, events, initial, paths, exit_with_last)`: el bucle genérico (backend de
  ratatui + flujo de eventos) que usan `run`, `run_app` y las pruebas.
- `Paths` gana `sessions_dir` y `Paths::default_paths()`.
- Eliminados `shell_session`, `connect_ui` y `Connected` (la conexión es ahora por panel, en
  segundo plano). `friki-cli` solo usaba `run`.

## Códigos de salida

- Un solo panel abierto en total: el de ese panel (código remoto, `Network` si se pierde, el
  error de conexión sin envolver, 130 si se cancela con Ctrl+C mientras conecta).
- Varios paneles abiertos: 0 (por `quit` o al cerrar el último). Fuente de eventos cerrada: 1.

## Pendiente de sustituir / otras oleadas

- `keys::parse_chord_local` es copia privada de la semántica de `friki_term::parse_chord`
  (que en esta rama es `todo!()`): sustituir por la de friki-term y borrar la local.
- `AppConfig::default()` se usa para prefijo y atajos; cuando `load_app_config` esté
  implementado, cargar `paths::app_config_file()` en `App::new`.
- Perfiles de terminal (`Terminal::with_profile`) no se aplican: los paneles usan
  `Emulator::new(filas, columnas, SCROLLBACK)`.
- Las acciones `search` y `copy-mode` están reconocidas y muestran "pendiente" en la barra.

## Notas

- Atajos: normalización de teclas (letras en minúscula con `shift` si eran mayúsculas; símbolos
  sin `shift`; `Ctrl+5` equivale a `Ctrl+]`), así que `shift+o` coincide con `O`.
- `split-horizontal` apila los paneles (arriba/abajo, como `"` de tmux); `split-vertical` los
  pone lado a lado (`%`). La división horizontal no gasta una fila de separador: el título del
  panel de abajo hace de línea.
- Las tareas de panel reenvían los eventos del shell por un canal sin límite al bucle central.

## Histórico

- Resuelto: `friki_ssh::Negotiated::weak()` es público y la TUI lo usa (ya no hay copia).
