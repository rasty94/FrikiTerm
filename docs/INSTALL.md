# Instalación de friki

Esta guía cubre la instalación desde las releases firmadas, cómo verificar lo descargado y la compilación desde el código fuente.

## 1. Qué publica cada release

Cada release de GitHub (`https://github.com/rasty94/frikiSSH/releases`) incluye:

| Fichero | Plataforma |
|---|---|
| `friki-vX.Y.Z-x86_64-unknown-linux-gnu.tar.gz` | Linux x86_64 |
| `friki-vX.Y.Z-aarch64-unknown-linux-gnu.tar.gz` | Linux aarch64 |
| `friki_X.Y.Z_amd64.deb` | Debian/Ubuntu x86_64 |
| `friki_X.Y.Z_arm64.deb` | Debian/Ubuntu aarch64 |
| `friki-vX.Y.Z-x86_64-apple-darwin.tar.gz` | macOS Intel |
| `friki-vX.Y.Z-aarch64-apple-darwin.tar.gz` | macOS Apple Silicon |
| `friki-vX.Y.Z-x86_64-pc-windows-msvc.zip` | Windows x86_64 |
| `SHA256SUMS` | Hashes SHA-256 de todos los ficheros anteriores |
| `*.sigstore.json` | Firma keyless (cosign) de cada fichero y de `SHA256SUMS` |

Cada fichero lleva además una atestación de procedencia de GitHub (SLSA) generada por el workflow `.github/workflows/release.yml`.

Los paquetes `.deb` no ejecutan scripts de mantenedor (ni al instalar ni al eliminar) y no se instalan como root más allá de lo que haga `dpkg`/`apt`.

## 2. Verificar antes de instalar

Conviene hacer los tres pasos: hash, firma y atestación. Sustituye `vX.Y.Z` y el nombre del fichero por los reales.

### 2.1 Firma de SHA256SUMS (cosign, keyless)

Instala [cosign](https://docs.sigstore.dev/cosign/system_config/installation/) y ejecuta:

```sh
cosign verify-blob \
  --bundle SHA256SUMS.sigstore.json \
  --certificate-identity "https://github.com/rasty94/frikiSSH/.github/workflows/release.yml@refs/tags/vX.Y.Z" \
  --certificate-oidc-issuer "https://token.actions.githubusercontent.com" \
  SHA256SUMS
```

La identidad no es una clave pública del proyecto: es el workflow de release de este repositorio, ejecutado sobre la etiqueta `vX.Y.Z`. Si quieres aceptar cualquier versión, cambia `--certificate-identity` por `--certificate-identity-regexp '^https://github.com/rasty94/frikiSSH/\.github/workflows/release\.yml@refs/tags/v'`.

### 2.2 Hash SHA-256

Con la firma de `SHA256SUMS` ya comprobada, verifica el fichero descargado contra esa lista:

```sh
# Linux
sha256sum -c SHA256SUMS --ignore-missing

# macOS
shasum -a 256 -c SHA256SUMS --ignore-missing
```

Debe aparecer `OK` para el fichero que vas a instalar.

### 2.3 Firma del fichero concreto (opcional)

```sh
cosign verify-blob \
  --bundle friki-vX.Y.Z-x86_64-unknown-linux-gnu.tar.gz.sigstore.json \
  --certificate-identity "https://github.com/rasty94/frikiSSH/.github/workflows/release.yml@refs/tags/vX.Y.Z" \
  --certificate-oidc-issuer "https://token.actions.githubusercontent.com" \
  friki-vX.Y.Z-x86_64-unknown-linux-gnu.tar.gz
```

### 2.4 Atestación de procedencia (GitHub CLI)

Requiere [`gh`](https://cli.github.com/) 2.49 o posterior:

```sh
gh attestation verify friki-vX.Y.Z-x86_64-unknown-linux-gnu.tar.gz --repo rasty94/frikiSSH
```

Esto confirma que el fichero se construyó en el workflow de este repositorio y no fuera de él.

## 3. Instalar

### Linux: paquete .deb

```sh
sudo apt install ./friki_X.Y.Z_amd64.deb
```

Usa `apt` con la ruta local (`./`) para que resuelva las dependencias declaradas. La lista de dependencias la calcula `cargo-deb` con `$auto` a partir de las librerías enlazadas (por ejemplo `libdbus-1-3` para el llavero del sistema).

La página man y las completions de bash, zsh y fish quedan instaladas en `/usr/share/man/man1/friki.1.gz` y en las rutas de completions habituales del sistema.

### Linux o macOS: tar.gz

```sh
tar -xzf friki-vX.Y.Z-<target>.tar.gz
install -m 755 friki-vX.Y.Z-<target>/friki ~/.local/bin/friki
```

Las completions van en `friki-vX.Y.Z-<target>/completions/`, y la página man en `friki-vX.Y.Z-<target>/friki.1` (en el tar.gz de Linux y macOS).

### Windows: zip

Descomprime `friki-vX.Y.Z-x86_64-pc-windows-msvc.zip` y ejecuta `friki.exe`. Para PowerShell, la completion es `completions\_friki.ps1`.

## 4. Compilar desde el código fuente

Requisitos:

- Rust: la versión queda fijada en `rust-toolchain.toml`. `rustup` la instala sola al entrar en el repositorio.
- Linux: `libdbus-1-dev` y `pkg-config` (keyring del sistema).
- macOS y Windows: ninguna dependencia de sistema adicional.

```sh
git clone https://github.com/rasty94/frikiSSH.git
cd frikiSSH
cargo build --release --locked -p friki-cli
./target/release/friki --help
```

El binario queda en `target/release/friki` (`friki.exe` en Windows).

Para generar la página man y las completions (bash, zsh, fish, PowerShell):

```sh
cargo run --locked -p xtask -- assets
```

Los ficheros salen en `crates/friki-cli/assets/gen/` (`friki.1` y `completions/`), que están fuera de git.

Para generar el `.deb` localmente (solo Linux, con `dpkg-shlibdeps` disponible):

```sh
cargo install cargo-deb --locked --version 3.8.0
cargo run --locked -p xtask -- assets
cargo build --release --locked -p friki-cli
cargo deb --no-build -p friki-cli --output target/debian/
```

En macOS y Windows el `.deb` no se puede generar: `$auto` necesita `dpkg-shlibdeps`.
