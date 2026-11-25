# FrikiTerm: TODO

Cliente SSH/terminal y transferencia de archivos en Rust, inspirado en las capacidades de SecureCRT/SecureFX documentadas en `docs/`.

## Estado (2026-11-25)

**Fases 1 a 5 hechas.** 392 pruebas (más 51 con la feature `legacy`), incluidas las de integración contra OpenSSH y telnetd reales en Docker; `cargo deny` y `cargo audit -D warnings` limpios; CI en Linux, macOS, Windows, MSRV 1.89 y `legacy`. Cuatro pasadas de revisión (corrección con Opus y seguridad con Fable, tras la fase 1 y al final), con todos los hallazgos altos y medios corregidos.

- **Fase 1**: SSH-2 (TUI y `exec`), claves, agente, `known_hosts`, algoritmos seguros con strict KEX, sesiones TOML, `ssh_config`, llavero, log con enmascarado, `keygen`.
- **Fase 2**: pestañas y paneles, atajos configurables (`config.toml`), perfiles y emulaciones (xterm, VT100, VT220, Linux, ANSI), temas, búsqueda, modo copia (portapapeles u OSC 52), selector por carpetas, emulador sin diferencias conocidas frente a xterm en el banco golden.
- **Fase 3**: SFTP, SCP (nombre esperado validado), FTP/FTPS; cola con paralelismo, reintentos, reconexión, reanudación sobre `.friki-part` y límite de ancho de banda; sincronización con filtros, conflictos y tope de borrados; CLI `ls/get/put/cp/sync/ftp`.
- **Fase 4**: ProxyJump, `-L/-R/-D`, reenvío de agente (solo en shell interactivo), exigir strict KEX, Telnet/TLS, RAW, serie, shell local, X/Y/Zmodem en paneles, scripting Rhai cancelable con sandbox, `pubkey-install` atómico.
- **Fase 5**: release firmada (cosign, atestación) tras tests/deny/audit, `.deb`, SHA256SUMS, man y completions, `docs/INSTALL.md`, licencias MIT/Apache-2.0.

**Pendiente:**
- [ ] Probar a mano la TUI en una terminal real en las tres plataformas (las pruebas usan un backend simulado).
- [ ] Sin probar en real: puerto serie con hardware (la prueba con par PTY corre en el CI de Linux), FTP contra un servidor real, el caso negativo de strict KEX, interoperabilidad Zmodem con `rz/sz` fuera del contenedor. (ConPTY sí se prueba en el CI de Windows.)
- [ ] El emulador no responde a consultas de la terminal (DSR, DA). En Windows, `friki-pty` responde a la de ConPTY al arrancar con una posición fija `1;1`.
- [ ] Agente SSH en Windows (Pageant / named pipe).
- [ ] Mantenedor del `.deb`: hoy `FrikiTerm <noreply@invalid>`.
- [ ] Crear el entorno `release` con revisores y proteger los tags `v*` en GitHub (`docs/INSTALL.md` §5).
- [ ] Revisar las excepciones de `deny.toml` / `.cargo/audit.toml` antes del 2027-05-25.
- [ ] Fecha del nombre del log en UTC (`time` no da el desfase local con varios hilos).
- [ ] Reenvío del ratón al remoto; Kermit; TN3270, Wyse y TVI (fuera del alcance actual).
- [ ] `sync --both` sin caché en la CLI; `cp` descarga un solo origen remoto.

## 0. Reglas del proyecto

- [ ] **Sala limpia.** Se implementa desde RFC y documentación pública. No se copia ni se traduce código de los binarios de VanDyke; los informes de `docs/` describen *qué hace* el producto, no *cómo*. No hay nada de licencias, ofuscación ni credenciales ajenas.
- [ ] **Reutilizar antes que escribir.** Ninguna criptografía ni parser de protocolo propios si hay crate mantenido (ver sección 12).
- [ ] **Seguro por defecto.** Los puntos débiles observados en `docs/03`, `07`, `09`, `14` y `15` son requisitos negativos (sección 11).
- [ ] Licencia del repositorio decidida y declarada (`LICENSE`).
- [ ] CI: `cargo fmt --check`, `cargo clippy -D warnings`, `cargo test`, `cargo deny`, `cargo audit`.
- [ ] MSRV fijada; builds para macOS (arm64/x86_64), Linux (x86_64/aarch64) y Windows (x86_64).

## 1. Decisiones abiertas (resolver antes de la fase 2)

- [ ] **Interfaz:** TUI (ratatui) primero, GUI después (egui/Tauri) o solo CLI+TUI. Recomendación: TUI, porque el núcleo es el mismo y se valida antes.
- [ ] **Scripting:** Lua, Rhai o Python embebido. SecureCRT usa Python (`docs/12`); un Python embebido arrastra su ciclo de CVEs (`docs/14`).
- [ ] **Formato de configuración:** TOML por sesión, o directorio de sesiones compatible con `~/.ssh/config`.
- [ ] **Alcance de compatibilidad:** ¿importar sesiones de SecureCRT/OpenSSH? ¿qué se considera "compatible"?

## 2. Núcleo SSH-2 (ver `docs/01`, `05`, `06`, `09`, `10`)

- [ ] Conexión, transporte y canales sobre un crate SSH existente (p. ej. `russh`); sin pila propia.
- [ ] **KEX:** curve25519-sha256 (y @libssh.org), ECDH NIST P-256/384/521, DH grupo 14-18 sha256/512, group-exchange-sha256.
- [ ] **Cifrados:** AES-GCM, AES-CTR, ChaCha20-Poly1305.
- [ ] **MAC:** HMAC-SHA2 (con variantes etm).
- [ ] **Claves de host y de usuario:** Ed25519, ECDSA (P-256/384/521), RSA con rsa-sha2-256/512; certificados OpenSSH.
- [ ] **Strict KEX** (mitigación Terrapin) y `ext-info`.
- [ ] Autenticación: password, publickey, keyboard-interactive, agente (`SSH_AUTH_SOCK`/Pageant/OpenSSH agent), GSSAPI (opcional, fase 4), PKCS#11 (opcional, fase 4).
- [ ] Orden de métodos configurable y número máximo de intentos explícito (SecureCRT no lo documenta: `docs/10` no se pudo escribir).
- [ ] Compresión `zlib@openssh.com` opcional, desactivada por defecto.
- [ ] Verificación de claves de host: `known_hosts` propio y compatible con OpenSSH, avisos ante cambio de clave, nunca aceptar en silencio.
- [ ] Reenvío de puertos local/remoto/dinámico (SOCKS), X11 y agente (opt-in).
- [ ] Keepalive, reconexión y timeouts configurables.
- [ ] Jump hosts / ProxyJump y proxy HTTP/SOCKS.
- [ ] Algoritmos heredados (3DES, CBC, SHA-1, DH group1, DSA, RC4) **fuera por defecto**; solo accesibles con una opción explícita por sesión que lo registre en el log.

## 3. Terminal (ver `docs/16`)

- [ ] Emulación basada en un crate existente (`alacritty_terminal`, `vt100` o `termwiz`); valorar cobertura antes de escribir nada.
- [ ] Emulaciones objetivo: xterm-256color, VT100/102/220, ANSI, Linux, SCO ANSI; después Wyse 50/60 y TVI si hay demanda.
- [ ] TN3270 como módulo opcional y aparte.
- [ ] Teclado: mapeo configurable, teclas de función, modos de aplicación, bracketed paste.
- [ ] Colores, fuentes, conjuntos de caracteres y UTF-8 correcto.
- [ ] Pestañas, paneles divididos, buscar en el buffer, scrollback configurable.
- [ ] Registro de sesión con variables en el nombre. **Enmascarado de contraseñas en el log** (requisito negativo: SecureCRT no lo documenta).

## 4. Otros tipos de sesión (ver `docs/01`, `16`)

- [ ] Local shell (pty).
- [ ] Telnet (NAWS, LFLOW, SGA, terminal type) y Telnet sobre TLS con verificación activa.
- [ ] Puerto serie: baud, paridad, bits, parada, control de flujo XON/XOFF y RTS/CTS (DTR/DSR no existe en macOS/Linux, `docs/16`).
- [ ] RAW (host y puerto).
- [ ] Transferencia en terminal: Xmodem, Ymodem, Zmodem; Kermit opcional.

## 5. Transferencia de archivos (ver `docs/13`)

- [ ] SFTP y SCP; FTP y FTPS (verificación de certificado activa); HTTP/S; S3 (SigV4, STS, token de sesión) como opcional.
- [ ] OneDrive **no** es un protocolo del producto de referencia (`docs/13`); no se implementa.
- [ ] Cola de transferencias: pausar, reanudar (ahora/luego/nunca), reintentos, reconexión, paralelismo y límite de ancho de banda.
- [ ] Sincronización de carpetas con comparación de directorios y filtros; caché de directorios (SQLite, esquema simple: `config` y `direntries`).
- [ ] Servidor TFTP opcional.

## 6. CLI y automatización (ver `docs/13`)

- [ ] Binario principal `friki` con subcomandos: `ssh`, `sftp`, `scp`, `keygen`, `pubkey-assist`, `sync`.
- [x] Códigos de salida **que funcionen** y estén documentados: 0 / 250 uso / 251 red / 252 autenticación / 253 clave de host / 254 otros (`ARCHITECTURE.md`). El esquema 100-400 de `sfxcl` no cabe en 8 bits en Unix, y el ejemplo `if not errorlevel 0` de su manual no detecta fallos nunca.
- [ ] Salida `--json` para scripts.
- [ ] Generación de claves (Ed25519 por defecto) y gestión de claves públicas.

## 7. Scripting (ver `docs/12`)

- [ ] Lenguaje decidido (sección 1) y API pública mínima equivalente: `Application`, `Session`, `Screen`, `Tab`, `FileTransfer`, `Dialog`, `GlobalConfiguration`, `SessionConfiguration`.
- [ ] Operaciones de pantalla: `Send`, `WaitForString(s)`, `ReadString`, `Get`, lectura de coordenadas; con timeouts obligatorios.
- [ ] Sandbox: sin red ni disco por defecto; permisos explícitos por script.
- [ ] Sin ejemplos con contraseñas literales ni HTML sin sanear (`docs/15`).

## 8. Sesiones y configuración

- [ ] Gestor de sesiones (carpetas, plantillas, herencia de opciones).
- [ ] Contraseñas y passphrases en el llavero del sistema (Keychain, Secret Service, Credential Manager); nunca en texto plano ni con cifrado propio.
- [ ] Importar `~/.ssh/config` y `known_hosts`; exportar a OpenSSH.
- [ ] Configuración con esquema versionado y migraciones.

## 9. TLS (ver `docs/07`)

- [ ] `rustls` por defecto; sin SSLv3 ni TLS 1.0/1.1; mínimo TLS 1.2.
- [ ] Verificación de certificado **siempre** con comprobación de hostname; sin nombre de destino vacío que la omita.
- [ ] SNI y ALPN activados.
- [ ] Revocación (OCSP/CRL) opcional y configurable.

## 10. Pruebas

- [ ] Unitarias por módulo y de integración contra un `sshd` real (OpenSSH) en contenedor.
- [ ] Servidores de prueba con algoritmos concretos: debe fallar al negociar uno débil.
- [ ] Fuzzing (`cargo-fuzz`) de los parsers que escribamos (secuencias de escape, Zmodem, formato de sesiones).
- [ ] Prueba específica de strict KEX y de cambio de clave de host.
- [ ] Pruebas de terminal con *golden files* de secuencias conocidas (vttest).

## 11. Requisitos negativos: lo que el producto de referencia hace peor o no documenta

- [ ] Hostname TLS no comprobado con destino vacío (`docs/07`): aquí es imposible.
- [ ] Mínimo de versión TLS decidido por flags del llamador: aquí es una constante.
- [ ] Algoritmos débiles registrados en el código (RC4, 3DES, CBC, MD5, SHA-1, DH group1, DSA) sin garantía de que estén desactivados por defecto: aquí **nunca se ofrecen** salvo `legacy_algorithms` con `--features legacy`, y una prueba falla si la lista segura contiene `none`, `-cbc`, `sha1`, `md5`, `group1` o `dss`. Nota: `russh` compila siempre CBC, SHA-1 y DH group1; la feature `legacy` solo añade 3DES y DSA, así que la barrera es la lista ofrecida, no la compilación.
- [ ] Librerías propias con RELRO parcial (`docs/03`): compilar con RELRO completo, PIE y `-C control-flow-guard` en Windows.
- [ ] Dependencias fuera de soporte empaquetadas (Python 2.7, zlib 1.2.11, `docs/15`): `cargo deny` bloquea las vulnerables y sin mantenimiento.
- [ ] Ejecutable sin firma por binario en Linux: firmar releases (cosign) y publicar SHA-256.
- [ ] `Depends:` incompleto en el `.deb` (`docs/03`): empaquetado con dependencias completas o estático.
- [ ] Scripts de instalación que no verifican nada: sin scripts de mantenedor ni ejecución como root.

## 12. Dependencias candidatas (a validar con `cargo deny`)

| Necesidad | Candidato |
|---|---|
| SSH | `russh`, `russh-keys` (alternativa: `ssh2` si se acepta libssh2) |
| SFTP | `russh-sftp` |
| Async | `tokio` |
| TLS | `rustls`, `webpki-roots` / `rustls-native-certs` |
| TUI | `ratatui`, `crossterm` |
| Emulador de terminal | `alacritty_terminal`, `vt100`, `termwiz` |
| Serie | `serialport` |
| PTY | `portable-pty` |
| Claves/agente | `ssh-key`, `keyring` |
| Config | `serde`, `toml` |
| CLI | `clap` |
| SQLite | `rusqlite` |
| Scripting | `mlua` o `rhai` (según la decisión de la sección 1) |

## 13. Fases

1. **MVP:** SSH-2 interactivo, claves Ed25519/RSA, `known_hosts`, TUI con una pestaña, sesiones en TOML.
2. **Terminal completo:** pestañas, paneles, emulaciones, registro con enmascarado.
3. **Transferencia:** SFTP/SCP, cola, sincronización, FTP/FTPS.
4. **Extras:** scripting, serie, Telnet, Zmodem, GSSAPI/PKCS#11, jump hosts.
5. **Empaquetado:** releases firmadas, instaladores, documentación.

## 14. Criterios de aceptación globales

- [ ] Se conecta a un OpenSSH reciente y a uno antiguo (con `--features legacy`) con los algoritmos documentados.
- [ ] Ninguna opción de seguridad se degrada en silencio; toda excepción queda en el log.
- [ ] `cargo audit` y `cargo deny` limpios en CI.
- [ ] Cada requisito de este fichero enlaza a su prueba o a su issue.
