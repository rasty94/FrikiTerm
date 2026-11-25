# FrikiTerm

Cliente SSH/terminal propio, escrito desde cero a partir de los RFC y de los requisitos de [TODO.md](TODO.md) (sala limpia: no copia ni traduce código de ningún producto). Arquitectura y contrato entre crates en [ARCHITECTURE.md](ARCHITECTURE.md).

```sh
export PATH="/opt/homebrew/opt/rustup/bin:$PATH"
cargo run -p friki-cli -- ssh usuario@host          # sesión interactiva (Ctrl+] q para salir)
cargo run -p friki-cli -- exec usuario@host uname -a
cargo run -p friki-cli -- sessions import-ssh-config
```

### Referencia rápida de `friki`

`friki` sin argumentos abre la TUI con el selector de sesiones. `destino` es una sesión guardada o `[user@]host[:puerto]`; los remotos se escriben `destino:ruta` (como scp; IPv6 entre corchetes).

| Orden | Qué hace |
|---|---|
| `ssh destino` / `exec destino cmd` | Sesión en la TUI / orden remota sin TUI (`--json`) |
| `fwd destino -L … -R … -D …` | Mantiene las redirecciones hasta Ctrl+C y las imprime |
| `ls destino:ruta` | Lista por SFTP (`--json`) |
| `get destino:ruta [local]` · `put local… destino:ruta` | Descarga / sube por SFTP (`-r`) |
| `cp [-r] [--scp] origen… destino` | Estilo scp; SFTP por defecto, `--scp` fuerza SCP |
| `sync local destino:ruta [--down\|--both] [--delete] [--include g] [--exclude g] [--dry-run]` | Sincroniza; `--dry-run` imprime el plan |
| `ftp ls\|get\|put host[:puerto] … [--tls] [-u usuario]` | FTP/FTPS; contraseña en `FRIKI_FTP_PASSWORD` o por prompt |
| `pubkey-install destino [-i clave.pub]` | Como `ssh-copy-id`, idempotente (`-I` = clave para autenticarse) |
| `script f.rhai [--target destino] [--allow-read d] [--allow-write d] [--allow-connect] [-- args]` | Script Rhai sin TUI; `crt.Quit(n)` fija el código de salida |
| `telnet host[:puerto] [--tls]` · `serial dispositivo [--baud N …]` · `serial list` · `shell [orden]` | Otros protocolos en la TUI |
| `sessions list\|add\|import-ssh-config` · `keygen` | Sesiones y claves |

Opciones de conexión comunes: `-p`, `-l`, `-i`, `-J [user@]host[:puerto][,…]`, `-L/-R [bind:]puerto:host:puerto`, `-D [bind:]puerto`, `-A`, `--require-strict-kex`, `--legacy-algorithms`. Las transferencias aceptan `--json` (eventos JSON por línea en stdout) y muestran barra de progreso en stderr si es un TTY.

Códigos de salida: 0 correcto · 250 uso · 251 red · 252 autenticación · 253 clave de host · 254 otros (permisos o ruta inexistente incluidos). `man` y completions: `cargo run -p xtask -- assets`.

Pruebas contra un OpenSSH real en Docker:

```sh
eval "$(scripts/test-sshd.sh up)"
FRIKI_TEST_REQUIRED=1 cargo test --workspace -- --include-ignored
scripts/test-sshd.sh down
```

## FrikiTerm Desktop

Aplicación de escritorio (Tauri v2 + Svelte 5) en `apps/desktop`. Trabajo en curso: el backend y la UI se están completando por partes, así que todavía no hay binarios publicados.

```sh
cd apps/desktop
npm ci
npx tauri dev          # desarrollo, con recarga de la UI
npx tauri build        # empaquetado (dmg, deb, appimage, msi, nsis según plataforma)
```

En Linux hacen falta las dependencias de sistema de Tauri (`libwebkit2gtk-4.1-dev`, `librsvg2-dev`, `libdbus-1-dev`, `pkg-config`). Las comprobaciones de CI están en [`.github/workflows/desktop.yml`](.github/workflows/desktop.yml).
