# Cambios

## v0.1.0 (2026-11-25)

Primera versión.

- **SSH-2**: sesión interactiva (TUI) y `exec`; claves Ed25519/ECDSA/RSA, agente (Unix y Windows: pipes de OpenSSH y Pageant), keyboard-interactive, PKCS#11 y GSSAPI opcionales; `known_hosts` compatible con OpenSSH; algoritmos seguros con strict KEX y sin compresión; ProxyJump, `-L/-R/-D`, reenvío de agente solo en shell interactivo.
- **Terminal**: pestañas y paneles, atajos configurables, perfiles y emulaciones (xterm, VT100/220, Linux, ANSI, Wyse 50/60, TVI 910/925, TN3270), búsqueda, modo copia, respuestas DSR/DA y reenvío del ratón; log de sesión con enmascarado de contraseñas.
- **Otros protocolos**: Telnet y Telnet/TLS, RAW, puerto serie, shell local.
- **Transferencia**: SFTP, SCP, FTP/FTPS, HTTP/S (solo lectura) y S3; cola con reintentos, reanudación y límite de ancho de banda; `sync` con filtros, caché y protección ante raíces vacías; X/Y/Zmodem y Kermit en la terminal; servidor TFTP.
- **Automatización**: CLI con `--json`, scripting Rhai con sandbox, `pubkey-install`, `keygen`.
- **Distribución**: binarios para macOS (arm64/x86_64), Linux (x86_64/aarch64, `.deb`) y Windows (x86_64), firmados con cosign y con atestación de procedencia; página man y completions.
