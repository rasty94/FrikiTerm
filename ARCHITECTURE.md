# Arquitectura de FrikiTerm

Requisitos en [TODO.md](TODO.md). Decisiones tomadas: **TUI** (ratatui) primero, scripting con **Rhai** (fase 4), sesiones en **TOML** propio con importación de `~/.ssh/config`.

## Crates

```
friki-cli ──► friki-tui ──► friki-ssh ──► friki-core
    │             └───────► friki-term ─► friki-core
    └──────────────────────────────────► friki-core
```

| Crate | Responsabilidad | No hace |
|---|---|---|
| `friki-core` | Sesiones TOML, importar `ssh_config`, `known_hosts`, llavero, rutas | Red, terminal |
| `friki-ssh` | Conexión SSH-2 (`russh`), autenticación, política de algoritmos, shell, exec, generación de claves | Interfaz, prompts propios (los pide por traits) |
| `friki-term` | Emulación de terminal (bytes → `Screen`), codificación de teclas, registro de sesión con enmascarado | Red, dibujo |
| `friki-tui` | Dibujo, teclado, diálogos de clave de host y contraseña | Lógica de protocolo |
| `friki-cli` | Binario `friki`, subcomandos, códigos de salida | Lógica de negocio |

## Reglas para quien implementa

1. **Las firmas públicas de los `lib.rs` son el contrato.** Se pueden añadir funciones, campos privados y módulos; no se cambian ni se quitan firmas públicas. Si una firma impide hacer bien el trabajo, se documenta en `CONTRACT-CHANGES.md` en la raíz con la propuesta, y se sigue con la firma actual.
2. **Cada agente solo toca su crate** (y lo indicado en su tarea). Dependencias nuevas, solo en el `Cargo.toml` de su crate.
3. Sin criptografía propia ni parsers de protocolo propios cuando exista crate mantenido. Sala limpia: nada de los binarios de VanDyke.
4. Sin `unwrap()`/`expect()` en rutas de producción; sin `unsafe` salvo justificado en comentario.
5. `cargo fmt`, `cargo clippy --all-targets -- -D warnings` y `cargo test` del crate en verde antes de entregar.
6. Commits sin atribución a herramientas.

## Pruebas de integración SSH

Contra un OpenSSH en Docker (`docker/sshd-test/`, `scripts/test-sshd.sh`). Variables de entorno que exporta el script y leen las pruebas (marcadas `#[ignore]` si no están):

| Variable | Valor |
|---|---|
| `FRIKI_TEST_HOST` / `FRIKI_TEST_PORT` | `127.0.0.1` / `2222` (moderno) |
| `FRIKI_TEST_LEGACY_PORT` | `2223` (solo algoritmos heredados) |
| `FRIKI_TEST_NOSTRICT_PORT` | `2225` (OpenSSH 9.5p1 sin strict KEX; contenedor aparte, `Dockerfile.nostrict`) |
| `FRIKI_TEST_USER` / `FRIKI_TEST_PASSWORD` | `friki` / generada en cada arranque |
| `FRIKI_TEST_KEY` | ruta a una Ed25519 generada en cada arranque, autorizada en el servidor |
| `FRIKI_TEST_KNOWN_HOSTS` | `known_hosts` con la clave del servidor moderno |

## Códigos de salida de la CLI

0 correcto · 250 uso incorrecto · 251 red · 252 autenticación · 253 clave de host · 254 otros (incluye `Negotiation`). En `friki ssh` y `friki exec`, si el comando remoto termina con estado N, se devuelve N.

Los códigos caben en 8 bits: en Unix el estado de salida se trunca módulo 256, así que el esquema 100/200/300/400 de `sfxcl` (Windows) no es portable. Van al final del rango, como el 255 de OpenSSH, para no chocar con los códigos habituales de un comando remoto.

## Fase 2: terminal completo

Contratos nuevos (las firmas son las de los `lib.rs`; mismas reglas que en la fase 1):

| Crate | Añade |
|---|---|
| `friki-core` | `TerminalProfile` (emulación, scrollback, retroceso, tema, `keymap`) en `SessionConfig.terminal`; `SessionConfig.folder`; `AppConfig` con prefijo y atajos (`load_app_config`, `paths::app_config_file`); `session_tree`, `normalize_folder`, `parse_key_bytes` |
| `friki-term` | `parse_chord` (texto → `Key`+`Modifiers`, lo usan TUI y keymap); `Terminal::with_profile`, `term_name`, `profile_warnings`; búsqueda (`search`, `reveal_row`, `top_row`, `Match`) y texto para copiar (`text_range`, `Pos`) |
| `friki-tui` | Multiplexor: varias sesiones a la vez en pestañas y paneles divididos, foco, atajos configurables, apertura de sesiones |

Atajos por defecto (tras el prefijo `Ctrl+]`, estilo tmux): `c` pestaña nueva, `n`/`p` siguiente/anterior, `"` dividir horizontal, `%` dividir vertical, `o` siguiente panel, `x` cerrar panel, `s` sesiones, `,` renombrar pestaña, `/` buscar, `[` modo copia, `PgUp`/`PgDn` scroll, `?` ayuda, `q` salir.

## Fases 3 y 4: transferencia, protocolos y extras

```
friki-cli ─► friki-tui ─► friki-ssh, friki-telnet, friki-serial, friki-pty, friki-zmodem, friki-script
     └────► friki-xfer ─► friki-ssh
todos ─► friki-core (config, transport)
```

- `friki_core::transport`: `StreamWriter`/`StreamReader`/`Stream`, flujo común de SSH, Telnet, RAW, serie y PTY. La TUI trabaja solo con esto.
- `SessionConfig` gana `protocol`, `serial`, `local_command`, `forwards`, `forward_agent`, `require_strict_kex` (todos con valor por defecto: los TOML antiguos siguen cargando).
- `friki-ssh`: `open_shell_stream`, `open_subsystem`, `open_exec_stream` (`ChannelStream`), `start_forward`/`stop_forward`; ProxyJump dentro de `connect`.
- Crates nuevos: `friki-xfer` (trait `RemoteFs`, SFTP, SCP, FTP/FTPS, `LocalFs`, cola, sincronización, caché SQLite), `friki-telnet`, `friki-serial`, `friki-pty`, `friki-zmodem`, `friki-script` (Rhai con `ScriptHost`).
