# Fuzzing

Objetivos `cargo-fuzz` de los parsers propios que consumen datos no confiables.
Esta carpeta está fuera del workspace (necesita nightly; no afecta a stable ni a la MSRV).

| Objetivo       | Cubre |
|----------------|-------|
| `term_process` | preprocesador de secuencias de escape + `Terminal::process`, búsqueda y codificación de pegado |
| `zmodem`       | `detect` y recepción Xmodem/Ymodem/Zmodem con bytes arbitrarios (sin cuelgues ni escrituras fuera del destino) |
| `telnet`       | analizador y negociación de opciones Telnet |
| `core_files`   | config TOML, sesiones, importación de `ssh_config`, `known_hosts` |
| `xfer`         | `safe_name`, `local_target`, listados FTP y cabeceras SCP |

## Requisitos

```sh
rustup toolchain install nightly
cargo install cargo-fuzz
```

## Uso

```sh
cd fuzz
cargo +nightly fuzz build                                   # compila todos los objetivos
cargo +nightly fuzz run zmodem -- -max_total_time=60 -timeout=10
```

`-timeout=10` marca como fallo cualquier entrada que tarde más de 10 s.

## Fallos

Un fallo queda en `fuzz/artifacts/<objetivo>/`. Para reproducirlo y depurarlo:

```sh
cargo +nightly fuzz run <objetivo> fuzz/artifacts/<objetivo>/<fichero>
cargo +nightly fuzz tmin <objetivo> fuzz/artifacts/<objetivo>/<fichero>
```

Tras corregir la causa en el crate afectado, añade una prueba unitaria con la
entrada mínima. No se versionan `fuzz/target/`, `fuzz/corpus/` ni `fuzz/artifacts/`.

## CI

El job `fuzz` de `.github/workflows/ci.yml` compila todos los objetivos y ejecuta
cada uno 30 s. Si falla, sube `fuzz/artifacts` como artefacto del workflow.
