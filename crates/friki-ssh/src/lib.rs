//! Cliente SSH-2 sobre un crate existente (`russh`). Sin criptografía propia.

mod auth;
mod errors;
mod forward;
#[cfg(all(feature = "gssapi", unix))]
mod gssapi;
#[cfg(feature = "pkcs11")]
mod pkcs11;
mod policy;
mod session;
mod verifier;

use std::path::Path;
use std::sync::Arc;

use friki_core::config::SessionConfig;
use friki_core::known_hosts::HostKey;
pub use zeroize::Zeroizing;

pub use errors::Error;
pub use verifier::KnownHostsVerifier;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostKeyDecision {
    Reject,
    AcceptOnce,
    AcceptAndSave,
}

/// Decide sobre la clave del servidor. La implementación por defecto y única
/// (`KnownHostsVerifier`) consulta `known_hosts` y delega en `ask` lo desconocido.
#[async_trait::async_trait]
pub trait HostKeyVerifier: Send + Sync {
    async fn verify(&self, host: &str, port: u16, key: &HostKey) -> HostKeyDecision;

    /// Algoritmos de clave de host ya conocidos para `host:port` (p. ej. `ssh-ed25519`).
    /// Se ofrecen primero, como hace OpenSSH, para no negociar un algoritmo sin entrada.
    fn known_algorithms(&self, _host: &str, _port: u16) -> Vec<String> {
        Vec::new()
    }

    /// Avisos acumulados por el verificador (p. ej. fallo al guardar en known_hosts).
    fn warnings(&self) -> Vec<String> {
        Vec::new()
    }
}

/// Las contraseñas y passphrases viajan en `Zeroizing`: se borran al soltarse.
#[async_trait::async_trait]
pub trait AuthPrompter: Send + Sync {
    async fn password(&self, user: &str, host: &str) -> Option<Zeroizing<String>>;
    async fn passphrase(&self, key_path: &Path) -> Option<Zeroizing<String>>;
    /// PIN de un token PKCS#11 (por su etiqueta). Por defecto se pide como passphrase.
    async fn pin(&self, token_label: &str) -> Option<Zeroizing<String>> {
        self.passphrase(Path::new(token_label)).await
    }
    /// `prompts`: (texto, mostrar eco).
    async fn keyboard_interactive(
        &self,
        name: &str,
        instructions: &str,
        prompts: &[(String, bool)],
    ) -> Option<Vec<Zeroizing<String>>>;
    /// Igual que `keyboard_interactive` indicando qué servidor pregunta (el salto de
    /// ProxyJump o el destino). friki-ssh llama siempre a esta; por defecto delega.
    async fn keyboard_interactive_for(
        &self,
        host: &str,
        name: &str,
        instructions: &str,
        prompts: &[(String, bool)],
    ) -> Option<Vec<Zeroizing<String>>> {
        let _ = host;
        self.keyboard_interactive(name, instructions, prompts).await
    }
}

/// Algoritmos ofrecidos, en orden de preferencia (TODO.md §2 y §11).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AlgorithmPolicy {
    pub kex: Vec<&'static str>,
    pub ciphers: Vec<&'static str>,
    pub macs: Vec<&'static str>,
    pub host_keys: Vec<&'static str>,
}

impl AlgorithmPolicy {
    /// Sin 3DES, CBC, RC4, SHA-1, MD5, DH group1 ni DSA. Incluye strict KEX
    /// (las extensiones `ext-info-c` y `kex-strict-c-v00@openssh.com` se añaden
    /// al traducir a `russh::Preferred`; no son algoritmos y no figuran en `kex`).
    pub fn secure() -> Self {
        Self {
            kex: vec![
                "curve25519-sha256",
                "curve25519-sha256@libssh.org",
                "ecdh-sha2-nistp256",
                "ecdh-sha2-nistp384",
                "ecdh-sha2-nistp521",
                "diffie-hellman-group16-sha512",
                "diffie-hellman-group18-sha512",
                "diffie-hellman-group14-sha256",
                "diffie-hellman-group-exchange-sha256",
            ],
            ciphers: vec![
                "chacha20-poly1305@openssh.com",
                "aes256-gcm@openssh.com",
                "aes128-gcm@openssh.com",
                "aes256-ctr",
                "aes192-ctr",
                "aes128-ctr",
            ],
            macs: vec![
                "hmac-sha2-256-etm@openssh.com",
                "hmac-sha2-512-etm@openssh.com",
                "hmac-sha2-256",
                "hmac-sha2-512",
            ],
            host_keys: vec![
                "ssh-ed25519",
                "ecdsa-sha2-nistp256",
                "ecdsa-sha2-nistp384",
                "ecdsa-sha2-nistp521",
                "rsa-sha2-512",
                "rsa-sha2-256",
            ],
        }
    }

    /// `secure()` más los heredados, al final de cada lista. russh los soporta
    /// todos con las características `des` y `dsa` (que activa la feature `legacy`).
    #[cfg(feature = "legacy")]
    pub fn legacy() -> Self {
        let mut p = Self::secure();
        p.kex
            .extend(["diffie-hellman-group14-sha1", "diffie-hellman-group1-sha1"]);
        p.ciphers
            .extend(["aes256-cbc", "aes192-cbc", "aes128-cbc", "3des-cbc"]);
        p.macs.extend(["hmac-sha1-etm@openssh.com", "hmac-sha1"]);
        p.host_keys.extend(["ssh-rsa", "ssh-dss"]);
        p
    }

    /// `legacy()` solo si `cfg.legacy_algorithms` y la feature está activa.
    pub fn for_session(cfg: &SessionConfig) -> Self {
        #[cfg(feature = "legacy")]
        if cfg.legacy_algorithms {
            return Self::legacy();
        }
        let _ = cfg;
        Self::secure()
    }
}

/// Resultado de `Connection::exec`. Con `exit_signal` presente, `exit_status` es `None`.
pub struct ExecOutput {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub exit_status: Option<u32>,
    /// Señal que mató la orden (p. ej. `KILL`), si la hubo.
    pub exit_signal: Option<String>,
}

/// Cómo terminó una orden de `exec_streaming`. Nunca llegan ambos a `None`
/// (eso es un error de red).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExecStatus {
    pub exit_status: Option<u32>,
    pub exit_signal: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShellEvent {
    Data(Vec<u8>),
    ExitStatus(u32),
    /// El shell remoto murió por una señal.
    ExitSignal {
        signal: String,
        core_dumped: bool,
    },
    /// Cierre limpio del canal (tras el estado o la señal de salida).
    Closed,
    /// Canal cerrado sin estado de salida o por error de transporte (corte,
    /// keepalive agotado, desbordamiento de lectura...). Texto para el usuario.
    Lost(String),
}

/// Algoritmos realmente negociados en el último intercambio de claves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Negotiated {
    pub kex: String,
    pub cipher: String,
    pub mac: String,
    pub host_key: String,
}

impl Negotiated {
    /// Los que la política segura nunca ofrece (3DES, CBC, SHA-1, group1, DSA, ssh-rsa).
    pub fn weak(&self) -> Vec<String> {
        let mut w = Vec::new();
        for (kind, n) in [
            ("kex", &self.kex),
            ("cifrado", &self.cipher),
            ("mac", &self.mac),
            ("clave de host", &self.host_key),
        ] {
            let bad = ["3des", "-cbc", "sha1", "md5", "group1-", "dss"]
                .iter()
                .any(|d| n.contains(d))
                || n == "ssh-rsa";
            if bad {
                w.push(format!("algoritmo heredado negociado ({kind}): {n}"));
            }
        }
        w
    }
}

/// Canal SSH como `AsyncRead + AsyncWrite`.
pub struct ChannelStream {
    inner: std::pin::Pin<Box<dyn DuplexStream>>,
    exit: std::sync::Arc<std::sync::Mutex<Option<u32>>>,
}

trait DuplexStream: tokio::io::AsyncRead + tokio::io::AsyncWrite + Send {}
impl<T: tokio::io::AsyncRead + tokio::io::AsyncWrite + Send> DuplexStream for T {}

impl ChannelStream {
    /// Estado de salida del comando remoto, si ya llegó.
    pub fn exit_status(&self) -> Option<u32> {
        self.exit.lock().map(|g| *g).unwrap_or(None)
    }
}

impl tokio::io::AsyncRead for ChannelStream {
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        self.inner.as_mut().poll_read(cx, buf)
    }
}

impl tokio::io::AsyncWrite for ChannelStream {
    fn poll_write(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        self.inner.as_mut().poll_write(cx, buf)
    }
    fn poll_flush(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        self.inner.as_mut().poll_flush(cx)
    }
    fn poll_shutdown(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        self.inner.as_mut().poll_shutdown(cx)
    }
}

/// Identifica una redirección activa.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForwardHandle {
    pub id: u64,
    pub listen: std::net::SocketAddr,
}

pub struct Connection {
    // Orden de campos = orden de destrucción: primero la conexión al destino y al final los saltos.
    handle: Arc<russh::client::Handle<session::Handler>>,
    info: Arc<session::Info>,
    verifier: Arc<dyn HostKeyVerifier>,
    warnings: Vec<String>,
    target: (String, String, u16),
    forwards: std::sync::Mutex<ForwardState>,
    _jumps: Vec<russh::client::Handle<session::Handler>>,
}

#[derive(Default)]
struct ForwardState {
    next_id: u64,
    active: std::collections::HashMap<u64, forward::Active>,
    warnings: Vec<String>,
}

impl Connection {
    /// Con `cfg.proxy_jump` (`[user@]host[:port]`, lista separada por comas) conecta
    /// por los saltos; la clave del destino se verifica contra el destino.
    pub async fn connect(
        cfg: &SessionConfig,
        verifier: Arc<dyn HostKeyVerifier>,
        prompter: Arc<dyn AuthPrompter>,
    ) -> Result<Self> {
        let c = session::connect(cfg, verifier.clone(), prompter).await?;
        Ok(Self {
            handle: Arc::new(c.handle),
            info: c.info,
            verifier,
            warnings: c.warnings,
            target: (
                cfg.user
                    .clone()
                    .or_else(session::default_user)
                    .unwrap_or_default(),
                cfg.host.clone(),
                cfg.port,
            ),
            forwards: std::sync::Mutex::default(),
            _jumps: c.jumps,
        })
    }

    /// (usuario, host, puerto) efectivos del destino (no de los saltos).
    pub fn target(&self) -> (String, String, u16) {
        self.target.clone()
    }

    fn fwd(&self) -> std::sync::MutexGuard<'_, ForwardState> {
        self.forwards
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Avisos de la conexión (nada se degrada en silencio): uso de `legacy`,
    /// `legacy_algorithms` sin la feature, claves ignoradas por permisos, fallo al
    /// guardar en known_hosts y algoritmos heredados negociados.
    pub fn warnings(&self) -> Vec<String> {
        let mut w = self.warnings.clone();
        if let Some(n) = self.negotiated() {
            w.extend(n.weak());
        }
        w.extend(self.fwd().warnings.clone());
        w.extend(self.info.runtime_warnings());
        w.extend(self.verifier.warnings());
        w
    }

    /// Kex, cifrado, mac y clave de host negociados (el último intercambio).
    pub fn negotiated(&self) -> Option<Negotiated> {
        self.info.negotiated()
    }

    /// Canal interactivo con PTY.
    pub async fn open_shell(
        &self,
        term: &str,
        cols: u16,
        rows: u16,
    ) -> Result<(ShellWriter, ShellReader)> {
        session::open_shell(&self.handle, &self.info, term, cols, rows).await
    }

    /// Ejecuta `command` sin stdin (se envía EOF de inmediato) y acumula la salida.
    /// Un cierre sin estado ni señal es `Err(Error::Network)`; una señal da
    /// `exit_status: None` y `exit_signal: Some(..)`.
    pub async fn exec(&self, command: &str) -> Result<ExecOutput> {
        let (mut stdout, mut stderr) = (Vec::new(), Vec::new());
        let st = session::exec_stream(
            &self.handle,
            &self.info,
            command,
            tokio::io::empty(),
            &mut stdout,
            &mut stderr,
        )
        .await?;
        Ok(ExecOutput {
            stdout,
            stderr,
            exit_status: st.exit_status,
            exit_signal: st.exit_signal,
        })
    }

    /// Como `exec`, pero en streaming: `stdin` se copia al remoto (EOF al agotarse;
    /// pasa `tokio::io::empty()` si no hay entrada) y la salida se escribe en
    /// `stdout`/`stderr` según llega. Mismos errores que `exec`; un fallo de E/S
    /// local es `Error::Other`.
    pub async fn exec_streaming<I, O, E>(
        &self,
        command: &str,
        stdin: I,
        stdout: O,
        stderr: E,
    ) -> Result<ExecStatus>
    where
        I: tokio::io::AsyncRead + Unpin,
        O: tokio::io::AsyncWrite + Unpin,
        E: tokio::io::AsyncWrite + Unpin,
    {
        session::exec_stream(&self.handle, &self.info, command, stdin, stdout, stderr).await
    }

    /// Shell interactivo como flujo genérico de `friki_core::transport` (para los paneles
    /// de la TUI, que mezclan SSH, Telnet, serie y PTY). `ExitStatus` → `Exit(n)`,
    /// `ExitSignal` → `Exit(exit_code_for_signal)`, `Lost` → `Lost`.
    pub async fn open_shell_stream(
        &self,
        term: &str,
        cols: u16,
        rows: u16,
    ) -> Result<friki_core::transport::Stream> {
        let (w, r) = self.open_shell(term, cols, rows).await?;
        Ok((Arc::new(w), Box::new(r)))
    }

    /// Canal con un subsistema (`"sftp"`) como flujo de bytes bidireccional.
    pub async fn open_subsystem(&self, name: &str) -> Result<ChannelStream> {
        session::open_channel_stream(&self.handle, session::Req::Subsystem(name)).await
    }

    /// Canal `exec` como flujo de bytes (para `scp -t`/`scp -f`). El estado de salida
    /// se obtiene con `ChannelStream::exit_status` tras el EOF.
    pub async fn open_exec_stream(&self, command: &str) -> Result<ChannelStream> {
        session::open_channel_stream(&self.handle, session::Req::Exec(command)).await
    }

    /// Activa una redirección de puertos (fase 4). Devuelve la dirección real de escucha
    /// (útil con puerto 0). Se mantiene mientras viva la conexión o hasta `stop_forward`.
    pub async fn start_forward(&self, fwd: &friki_core::config::Forward) -> Result<ForwardHandle> {
        let (listen, active, warn) = forward::start(&self.handle, &self.info, fwd).await?;
        let mut st = self.fwd();
        st.next_id += 1;
        let id = st.next_id;
        st.active.insert(id, active);
        st.warnings.extend(warn);
        Ok(ForwardHandle { id, listen })
    }

    pub async fn stop_forward(&self, handle: ForwardHandle) -> Result<()> {
        // Al soltar `Active::Task` se aborta la escucha local.
        let Some(active) = self.fwd().active.remove(&handle.id) else {
            return Ok(());
        };
        if let forward::Active::Remote { addr, port } = &active {
            self.info
                .remote_forwards
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .remove(&(addr.clone(), *port));
            self.handle
                .cancel_tcpip_forward(addr.clone(), *port)
                .await
                .map_err(|e| errors::classify(e, false))?;
        }
        Ok(())
    }

    pub async fn disconnect(self) -> Result<()> {
        self.handle
            .disconnect(russh::Disconnect::ByApplication, "", "en")
            .await
            .map_err(|e| errors::classify(e, false))
    }
}

/// Extremo de escritura; clonable para usarlo desde varias tareas. Nunca espera
/// a que se lea la salida: encola en una cola propia y, si se llena, da error.
#[derive(Clone)]
pub struct ShellWriter {
    tx: tokio::sync::mpsc::Sender<session::ShellCmd>,
}

impl ShellWriter {
    pub async fn write(&self, data: &[u8]) -> Result<()> {
        self.push(session::ShellCmd::Data(data.to_vec()))
    }

    pub async fn resize(&self, cols: u16, rows: u16) -> Result<()> {
        self.push(session::ShellCmd::Resize(cols, rows))
    }

    fn push(&self, cmd: session::ShellCmd) -> Result<()> {
        use tokio::sync::mpsc::error::TrySendError;
        self.tx.try_send(cmd).map_err(|e| match e {
            TrySendError::Full(_) => Error::Other("cola de escritura llena".into()),
            TrySendError::Closed(_) => Error::Network("canal cerrado".into()),
        })
    }
}

pub struct ShellReader {
    rx: tokio::sync::mpsc::UnboundedReceiver<ShellEvent>,
    queued: Arc<std::sync::atomic::AtomicUsize>,
}

impl ShellReader {
    /// `None` cuando el canal ya está cerrado (tras `Closed` o `Lost`).
    pub async fn next_event(&mut self) -> Option<ShellEvent> {
        let ev = self.rx.recv().await?;
        if let ShellEvent::Data(d) = &ev {
            self.queued
                .fetch_sub(d.len(), std::sync::atomic::Ordering::SeqCst);
        }
        Some(ev)
    }
}

fn to_io(e: Error) -> std::io::Error {
    let kind = match e {
        Error::Network(_) => std::io::ErrorKind::BrokenPipe,
        _ => std::io::ErrorKind::Other,
    };
    std::io::Error::new(kind, e.to_string())
}

#[async_trait::async_trait]
impl friki_core::transport::StreamWriter for ShellWriter {
    async fn write(&self, data: &[u8]) -> std::io::Result<()> {
        ShellWriter::write(self, data).await.map_err(to_io)
    }

    async fn resize(&self, cols: u16, rows: u16) -> std::io::Result<()> {
        ShellWriter::resize(self, cols, rows).await.map_err(to_io)
    }
}

#[async_trait::async_trait]
impl friki_core::transport::StreamReader for ShellReader {
    async fn next_event(&mut self) -> Option<friki_core::transport::StreamEvent> {
        use friki_core::transport::StreamEvent as E;
        Some(match ShellReader::next_event(self).await? {
            ShellEvent::Data(d) => E::Data(d),
            ShellEvent::ExitStatus(n) => E::Exit(i32::try_from(n).unwrap_or(i32::MAX)),
            ShellEvent::ExitSignal { signal, .. } => E::Exit(exit_code_for_signal(&signal)),
            ShellEvent::Closed => E::Closed,
            ShellEvent::Lost(why) => E::Lost(why),
        })
    }
}

/// Código de salida para un proceso remoto terminado por señal, como hacen los
/// shells: 128 + número POSIX. Señal desconocida = 255.
pub fn exit_code_for_signal(signal: &str) -> i32 {
    let n = match signal.trim_start_matches("SIG") {
        "HUP" => 1,
        "INT" => 2,
        "QUIT" => 3,
        "ILL" => 4,
        "ABRT" => 6,
        "FPE" => 8,
        "KILL" => 9,
        "USR1" => 10,
        "SEGV" => 11,
        "USR2" => 12,
        "PIPE" => 13,
        "ALRM" => 14,
        "TERM" => 15,
        _ => return 255,
    };
    128 + n
}

#[cfg(test)]
mod signal_tests {
    #[test]
    fn senales_conocidas_y_desconocidas() {
        assert_eq!(super::exit_code_for_signal("KILL"), 137);
        assert_eq!(super::exit_code_for_signal("SIGTERM"), 143);
        assert_eq!(super::exit_code_for_signal("RARA"), 255);
    }
}

pub mod keys {
    use std::io::Write;
    use std::path::{Path, PathBuf};

    use russh::keys::ssh_key::{Algorithm, LineEnding, PrivateKey};

    use super::{Error, Result};

    /// Genera una clave Ed25519 en formato OpenSSH (privada en `path`, 0600;
    /// pública en `path.pub`) y devuelve la línea pública. Falla si `path` ya existe.
    pub fn generate_ed25519(
        path: &Path,
        comment: &str,
        passphrase: Option<&str>,
    ) -> Result<String> {
        let other = |e: &dyn std::fmt::Display| Error::Other(format!("generar clave: {e}"));
        let mut rng = rand::rng();
        let mut key = PrivateKey::random(&mut rng, Algorithm::Ed25519).map_err(|e| other(&e))?;
        key.set_comment(comment);
        let public = key.public_key().to_openssh().map_err(|e| other(&e))?;

        let stored = match passphrase.filter(|p| !p.is_empty()) {
            Some(p) => key.encrypt(&mut rng, p).map_err(|e| other(&e))?,
            None => key,
        };
        let pem = stored.to_openssh(LineEnding::LF).map_err(|e| other(&e))?;

        // Directorio padre 0700 si hay que crearlo (como `ssh-keygen`).
        if let Some(dir) = path
            .parent()
            .filter(|d| !d.as_os_str().is_empty() && !d.exists())
        {
            let mut b = std::fs::DirBuilder::new();
            b.recursive(true);
            #[cfg(unix)]
            std::os::unix::fs::DirBuilderExt::mode(&mut b, 0o700);
            b.create(dir).map_err(|e| other(&e))?;
        }

        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut opts, 0o600);
        let mut f = opts.open(path).map_err(|e| other(&e))?;
        f.write_all(pem.as_bytes()).map_err(|e| other(&e))?;

        let mut pub_path = path.as_os_str().to_owned();
        pub_path.push(".pub");
        // `create_new`: no pisa una `.pub` existente.
        let mut pf = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(PathBuf::from(pub_path))
            .map_err(|e| other(&e))?;
        pf.write_all(format!("{public}\n").as_bytes())
            .map_err(|e| other(&e))?;
        Ok(public)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn genera_ed25519_con_y_sin_passphrase() {
        let dir = tempfile::tempdir().expect("tempdir");
        let plain = dir.path().join("id");
        let line = crate::keys::generate_ed25519(&plain, "c@h", None).expect("keygen");
        assert!(line.starts_with("ssh-ed25519 ") && line.ends_with(" c@h"));
        assert_eq!(
            std::fs::read_to_string(dir.path().join("id.pub"))
                .expect("pub")
                .trim(),
            line
        );
        assert!(russh::keys::load_secret_key(&plain, None).is_ok());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&plain)
                .expect("meta")
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        // No pisa una clave existente.
        assert!(crate::keys::generate_ed25519(&plain, "c", None).is_err());

        let enc = dir.path().join("enc");
        crate::keys::generate_ed25519(&enc, "c", Some("secreto")).expect("keygen cifrada");
        assert!(russh::keys::load_secret_key(&enc, None).is_err());
        assert!(russh::keys::load_secret_key(&enc, Some("secreto")).is_ok());
    }
}
