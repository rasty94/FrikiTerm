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
