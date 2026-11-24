# Arquitectura de frikiSSH

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
| `FRIKI_TEST_HOST` / `FRIKI_TEST_PORT` | `127.0.0.1` / `2222` (moderno), `2223` (solo algoritmos heredados) |
| `FRIKI_TEST_USER` / `FRIKI_TEST_PASSWORD` | `friki` / generada en cada arranque |
| `FRIKI_TEST_KEY` | ruta a una Ed25519 generada en cada arranque, autorizada en el servidor |
| `FRIKI_TEST_KNOWN_HOSTS` | `known_hosts` con la clave del servidor moderno |

## Códigos de salida de la CLI

0 correcto · 100 red · 200 autenticación · 300 clave de host · 400 otros (incluye `Negotiation`). En `friki ssh`, si el shell remoto termina con estado N, se devuelve N.
