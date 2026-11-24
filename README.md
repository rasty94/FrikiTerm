# securecrt

Análisis estático de SecureCRT/SecureFX 9.7.4 (VanDyke) con la skill `reverse-engineer-anything` (REA).

## Documentos

- [docs/00-arquitectura.md](docs/00-arquitectura.md): visión general
- [docs/01-protocolos-y-capacidades.md](docs/01-protocolos-y-capacidades.md)
- [docs/02-comparativa-plataformas.md](docs/02-comparativa-plataformas.md)
- [docs/03-auditoria-seguridad.md](docs/03-auditoria-seguridad.md)
- [docs/04-decompilacion-ssh2-init.md](docs/04-decompilacion-ssh2-init.md): primera decompilación (Ghidra)
- [docs/06-cifrados-mac-y-autenticacion.md](docs/06-cifrados-mac-y-autenticacion.md): cifrados, MAC y autenticación (parcial)
- [docs/07-tls-libtlscore.md](docs/07-tls-libtlscore.md): capa TLS y verificación de certificados
- [docs/09-listas-por-defecto-y-compresion.md](docs/09-listas-por-defecto-y-compresion.md): filtro de algoritmos y compresión
- [docs/12-api-de-scripting.md](docs/12-api-de-scripting.md): API de scripting Python, ayuda frente a binario
- [docs/13-transferencias-y-cli.md](docs/13-transferencias-y-cli.md): transferencias, sincronización y herramientas CLI
- [docs/14-terceros-y-cves.md](docs/14-terceros-y-cves.md): librerías de terceros y avisos públicos
- [docs/15-build-windows.md](docs/15-build-windows.md): hardening, firma y terceros del build de Windows
- [docs/16-terminal-telnet-serie.md](docs/16-terminal-telnet-serie.md): emulaciones, Telnet, serie y transferencias en terminal
- [docs/08-mapa-de-modulos.md](docs/08-mapa-de-modulos.md): módulos, clases y dependencias

## Reproducir

1. Colocar aquí el `.dmg`, el `.deb` y el `.exe` (ignorados por git) y extraerlos en `extracted/{macos,ubuntu,windows}`.
2. Registrar REA: `npx -y rea-agents@latest setup --client claude_code --yes`, y reiniciar Claude Code.
3. La skill vive en `.claude/skills/reverse-engineer-anything/`.

Solo análisis estático: no se ejecutó ningún binario del producto.

## REA + Ghidra en Docker

`docker/Dockerfile` construye una imagen con REA (MCP por stdio), JDK 25 y Ghidra 12.2 (SHA-256 verificado).

- **Plataforma:** se construye como `linux/amd64` (emulada en Mac arm64), porque Ghidra 12.2 solo trae decompilador nativo para `linux_x86_64` y `win_x86_64`.
- **JDK:** Ghidra 12.2 exige JDK 25 o superior.
- **Construir:** `docker build --platform linux/amd64 -t rea-ghidra -f docker/Dockerfile docker`
- **Registrar:** `claude mcp add -s user rea -- docker run -i --rm --platform linux/amd64 -v "$PWD:$PWD" rea-ghidra mcp` y reiniciar Claude Code.
- **Rutas:** el proyecto se monta en la misma ruta absoluta del host, porque REA devuelve rutas tal cual.
- **Comprobar:** `docker run --rm --platform linux/amd64 rea-ghidra doctor --json`

## Ghidra nativo en macOS arm64 (alternativa a Docker)

`scripts/build-ghidra-macos-arm64.sh` descarga la release de Ghidra 12.2 (SHA-256 verificado) y compila su decompilador desde el código fuente con `-arch arm64`. Instala en `~/tools/ghidra`; necesita `brew install openjdk` (JDK 25 o superior).

- **Resultado:** `rea doctor` da todas las comprobaciones de Ghidra en verde (`mach-o arm64`, JDK 27).
- **Prueba real:** decompilar `SSH2::Init` tarda unos 46 s en frío, frente a unos 2 min en Docker emulado, con el mismo resultado (mismos manejadores de KEX).
- **Uso:** `GHIDRA_INSTALL_DIR=~/tools/ghidra JAVA_HOME=/opt/homebrew/opt/openjdk npx -y rea-agents@latest decompile <binario> <dirección> --json`
- **Registro MCP actual:** `claude mcp add -s user rea -e GHIDRA_INSTALL_DIR=$HOME/tools/ghidra -e JAVA_HOME=/opt/homebrew/opt/openjdk -- npx -y rea-agents@latest mcp`. La opción de Docker de arriba sigue siendo válida si no quieres compilar nada.

## Cliente frikiSSH (Rust)

Cliente SSH/terminal propio, escrito desde cero a partir de los RFC y de los requisitos de [TODO.md](TODO.md) (sala limpia: no usa nada de los binarios analizados). Arquitectura y contrato entre crates en [ARCHITECTURE.md](ARCHITECTURE.md).

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
