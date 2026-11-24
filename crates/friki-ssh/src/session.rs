//! Conexión, canal de shell y exec sobre `russh::client`.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use friki_core::config::SessionConfig;
use friki_core::known_hosts::HostKey;
use russh::client::{self, DisconnectReason, Handle};
use russh::keys::PublicKeyOrCertificate;
use russh::{ChannelMsg, Preferred};
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt};
use tokio::sync::mpsc;

use crate::errors::classify;
use crate::{
    AlgorithmPolicy, AuthPrompter, Error, ExecStatus, HostKeyDecision, HostKeyVerifier, Negotiated,
    Result, ShellEvent, ShellReader, ShellWriter,
};

/// Cola de órdenes de escritura (datos y redimensionados) por shell.
const WRITE_QUEUE: usize = 256;
/// Salida del servidor pendiente de leer por la TUI antes de dar el canal por perdido.
/// La tarea lectora debe drenar siempre a russh (si no, la conexión entera se detiene).
pub(crate) const MAX_QUEUED_BYTES: usize = 32 * 1024 * 1024;

/// Estado compartido entre el `Handler` de russh y la `Connection`.
#[derive(Default)]
pub(crate) struct Info {
    lost: Mutex<Option<String>>,
    negotiated: Mutex<Option<Negotiated>>,
    /// `require_strict_kex`: abortar si el servidor no anuncia strict KEX.
    require_strict: AtomicBool,
    strict_failed: AtomicBool,
    /// `forward_agent`: pedir y atender el reenvío del agente.
    pub(crate) agent: AtomicBool,
    /// Redirecciones remotas: puerto escuchado en el servidor -> destino local.
    // ponytail: clave solo por puerto; dos binds distintos con el mismo puerto compartirían destino.
    pub(crate) remote_forwards: Mutex<HashMap<u32, (String, u16)>>,
}

impl Info {
    pub(crate) fn lost(&self) -> Option<String> {
        self.lost
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    pub(crate) fn negotiated(&self) -> Option<Negotiated> {
        self.negotiated
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    pub(crate) fn strict_failed(&self) -> bool {
        self.strict_failed.load(Ordering::SeqCst)
    }

    fn set_lost(&self, why: String) {
        *self.lost.lock().unwrap_or_else(PoisonError::into_inner) = Some(why);
    }
}

pub(crate) struct Handler {
    verifier: Arc<dyn HostKeyVerifier>,
    host: String,
    port: u16,
    rejected: Arc<AtomicBool>,
    /// `true` mientras el verificador (diálogo del usuario) tiene la palabra.
    in_dialog: Arc<AtomicBool>,
    info: Arc<Info>,
}

fn host_key_of(k: &PublicKeyOrCertificate) -> Option<HostKey> {
    let pk = match k {
        PublicKeyOrCertificate::PublicKey { key, .. } => key.clone(),
        PublicKeyOrCertificate::Certificate(c) => {
            russh::keys::PublicKey::from(c.public_key().clone())
        }
    };
    // Línea OpenSSH: "<algoritmo> <base64> [comentario]".
    let line = pk.to_openssh().ok()?;
    let mut parts = line.split_whitespace();
    Some(HostKey {
        algorithm: parts.next()?.to_string(),
        base64: parts.next()?.to_string(),
    })
}

impl client::Handler for Handler {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        key: &PublicKeyOrCertificate,
    ) -> std::result::Result<bool, Self::Error> {
        let Some(hk) = host_key_of(key) else {
            self.rejected.store(true, Ordering::SeqCst);
            return Ok(false);
        };
        self.in_dialog.store(true, Ordering::SeqCst);
        let d = self.verifier.verify(&self.host, self.port, &hk).await;
        self.in_dialog.store(false, Ordering::SeqCst);
        let ok = d != HostKeyDecision::Reject;
        if !ok {
            self.rejected.store(true, Ordering::SeqCst);
        }
        Ok(ok)
    }

    async fn kex_done(
        &mut self,
        _shared_secret: Option<&[u8]>,
        names: &russh::Names,
        _session: &mut client::Session,
    ) -> std::result::Result<(), Self::Error> {
        // Solo en el primer intercambio: en las reclaves el servidor no repite la extensión.
        let first = self.info.negotiated().is_none();
        if first && self.info.require_strict.load(Ordering::SeqCst) && !names.strict_kex() {
            self.info.strict_failed.store(true, Ordering::SeqCst);
            return Err(russh::Error::Kex);
        }
        *self
            .info
            .negotiated
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(Negotiated {
            kex: names.kex.as_ref().to_string(),
            cipher: names.cipher.as_ref().to_string(),
            mac: names.client_mac.as_ref().to_string(),
            host_key: names.key.to_string(),
        });
        Ok(())
    }

    async fn server_channel_open_forwarded_tcpip(
        &mut self,
        channel: russh::Channel<client::Msg>,
        _connected_address: &str,
        connected_port: u32,
        _originator_address: &str,
        _originator_port: u32,
        reply: client::ChannelOpenHandle,
        _session: &mut client::Session,
    ) -> std::result::Result<(), Self::Error> {
        let dest = self
            .info
            .remote_forwards
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&connected_port)
            .cloned();
        // Solo se atienden los puertos que pedimos nosotros.
        let Some((host, port)) = dest else {
            reply
                .reject(russh::ChannelOpenFailure::AdministrativelyProhibited)
                .await;
            return Ok(());
        };
        match tokio::net::TcpStream::connect((host.as_str(), port)).await {
            Ok(mut local) => {
                reply.accept().await;
                tokio::spawn(async move {
                    let mut chan = channel.into_stream();
                    let _ = tokio::io::copy_bidirectional(&mut local, &mut chan).await;
                });
            }
            Err(_) => reply.reject(russh::ChannelOpenFailure::ConnectFailed).await,
        }
        Ok(())
    }

    async fn server_channel_open_agent_forward(
        &mut self,
        channel: russh::Channel<client::Msg>,
        reply: client::ChannelOpenHandle,
        _session: &mut client::Session,
    ) -> std::result::Result<(), Self::Error> {
        #[cfg(unix)]
        if self.info.agent.load(Ordering::SeqCst)
            && let Some(sock) = std::env::var_os("SSH_AUTH_SOCK").filter(|s| !s.is_empty())
            && let Ok(mut agent) = tokio::net::UnixStream::connect(sock).await
        {
            reply.accept().await;
            tokio::spawn(async move {
                let mut chan = channel.into_stream();
                let _ = tokio::io::copy_bidirectional(&mut agent, &mut chan).await;
            });
            return Ok(());
        }
        let _ = channel;
        reply
            .reject(russh::ChannelOpenFailure::AdministrativelyProhibited)
            .await;
        Ok(())
    }

    async fn disconnected(
        &mut self,
        reason: DisconnectReason<Self::Error>,
    ) -> std::result::Result<(), Self::Error> {
        match reason {
            DisconnectReason::ReceivedDisconnect(i) => {
                self.info
                    .set_lost(format!("el servidor cerró la conexión: {i:?}"));
                Ok(())
            }
            DisconnectReason::Error(e) => {
                self.info.set_lost(e.to_string());
                Err(e)
            }
        }
    }
}

fn default_user() -> Option<String> {
    ["USER", "USERNAME", "LOGNAME"]
        .iter()
        .find_map(|v| std::env::var(v).ok())
        .filter(|u| !u.is_empty())
}

pub(crate) struct Connected {
    pub handle: Handle<Handler>,
    pub info: Arc<Info>,
    pub warnings: Vec<String>,
    /// Conexiones a los saltos de ProxyJump; deben vivir tanto como `handle`.
    pub jumps: Vec<Handle<Handler>>,
}

/// Un salto de `ProxyJump`: `[user@]host[:port]`.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Hop {
    pub user: Option<String>,
    pub host: String,
    pub port: u16,
}

pub(crate) fn parse_jumps(spec: &str) -> Result<Vec<Hop>> {
    let bad = |s: &str| Error::Other(format!("ProxyJump inválido: {s:?}"));
    spec.split(',')
        .map(str::trim)
        .map(|part| {
            let (user, rest) = match part.rsplit_once('@') {
                Some((u, r)) if !u.is_empty() => (Some(u.to_string()), r),
                Some(_) => return Err(bad(part)),
                None => (None, part),
            };
            let (host, port) = if let Some(r) = rest.strip_prefix('[') {
                // IPv6 entre corchetes: [::1] o [::1]:22
                let (h, tail) = r.split_once(']').ok_or_else(|| bad(part))?;
                match tail {
                    "" => (h, None),
                    t => (h, Some(t.strip_prefix(':').ok_or_else(|| bad(part))?)),
                }
            } else {
                match rest.split_once(':') {
                    Some((h, p)) => (h, Some(p)),
                    None => (rest, None),
                }
            };
            let port = match port {
                Some(p) => p.parse().map_err(|_| bad(part))?,
                None => 22,
            };
            if host.is_empty() {
                return Err(bad(part));
            }
            Ok(Hop {
                user,
                host: host.to_string(),
                port,
            })
        })
        .collect()
}

pub(crate) async fn connect(
    cfg: &SessionConfig,
    verifier: Arc<dyn HostKeyVerifier>,
    prompter: Arc<dyn AuthPrompter>,
) -> Result<Connected> {
    let hops = match cfg.proxy_jump.as_deref().map(str::trim) {
        Some(s) if !s.is_empty() => parse_jumps(s)?,
        _ => Vec::new(),
    };

    let mut warnings = Vec::new();
    if cfg.legacy_algorithms {
        warnings.push(
            if cfg!(feature = "legacy") {
                "algoritmos heredados habilitados (3DES, CBC, SHA-1, group1, DSA): conexión debilitada"
            } else {
                "legacy_algorithms está activo pero friki-ssh se compiló sin la feature `legacy`: \
                 solo se ofrecen algoritmos seguros"
            }
            .to_string(),
        );
    }
    if cfg.forward_agent {
        warnings.push(
            if cfg!(unix) {
                "reenvío del agente activo: un administrador del servidor podría usar tus claves mientras dure la sesión"
            } else {
                "forward_agent no está soportado en esta plataforma: no se reenvía el agente"
            }
            .to_string(),
        );
    }

    // Cada salto se conecta con los datos de la sesión (mismo verificador y prompter,
    // mismas credenciales salvo el usuario si el salto trae uno); sin sesiones guardadas.
    let mut jumps: Vec<Handle<Handler>> = Vec::new();
    for hop in &hops {
        let hop_cfg = SessionConfig {
            host: hop.host.clone(),
            port: hop.port,
            user: hop.user.clone().or_else(|| cfg.user.clone()),
            proxy_jump: None,
            forwards: Vec::new(),
            forward_agent: false,
            ..cfg.clone()
        };
        let (handle, _) = match jumps.last() {
            None => {
                let stream = tcp(&hop_cfg).await?;
                handshake(&hop_cfg, stream, &verifier, &prompter, &mut warnings).await?
            }
            Some(prev) => {
                let stream = through(prev, &hop_cfg).await?;
                handshake(&hop_cfg, stream, &verifier, &prompter, &mut warnings).await?
            }
        };
        jumps.push(handle);
    }

    let (handle, info) = match jumps.last() {
        None => {
            let stream = tcp(cfg).await?;
            handshake(cfg, stream, &verifier, &prompter, &mut warnings).await?
        }
        Some(prev) => {
            let stream = through(prev, cfg).await?;
            handshake(cfg, stream, &verifier, &prompter, &mut warnings).await?
        }
    };
    Ok(Connected {
        handle,
        info,
        warnings,
        jumps,
    })
}

fn timeout_of(cfg: &SessionConfig) -> Duration {
    Duration::from_secs(cfg.connect_timeout_secs.max(1))
}

fn timed_out(cfg: &SessionConfig) -> Error {
    Error::Network(format!(
        "tiempo de conexión agotado ({}s) con {}:{}",
        timeout_of(cfg).as_secs(),
        cfg.host,
        cfg.port
    ))
}

async fn tcp(cfg: &SessionConfig) -> Result<tokio::net::TcpStream> {
    tokio::time::timeout(
        timeout_of(cfg),
        tokio::net::TcpStream::connect((cfg.host.as_str(), cfg.port)),
    )
    .await
    .map_err(|_| timed_out(cfg))?
    .map_err(|e| Error::Network(format!("{}:{}: {e}", cfg.host, cfg.port)))
}

/// Canal `direct-tcpip` desde `via` hacia `cfg.host:cfg.port`, como flujo para el SSH interno.
async fn through(
    via: &Handle<Handler>,
    cfg: &SessionConfig,
) -> Result<russh::ChannelStream<client::Msg>> {
    let ch = tokio::time::timeout(
        timeout_of(cfg),
        via.channel_open_direct_tcpip(cfg.host.clone(), cfg.port.into(), "127.0.0.1", 0),
    )
    .await
    .map_err(|_| timed_out(cfg))?
    .map_err(|e| {
        Error::Network(format!(
            "el salto no pudo abrir {}:{}: {e}",
            cfg.host, cfg.port
        ))
    })?;
    Ok(ch.into_stream())
}

/// Handshake SSH sobre `stream`, verificación de la clave de `cfg.host:cfg.port` y autenticación.
async fn handshake<R>(
    cfg: &SessionConfig,
    stream: R,
    verifier: &Arc<dyn HostKeyVerifier>,
    prompter: &Arc<dyn AuthPrompter>,
    warnings: &mut Vec<String>,
) -> Result<(Handle<Handler>, Arc<Info>)>
where
    R: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let user = cfg
        .user
        .clone()
        .or_else(default_user)
        .ok_or_else(|| Error::Other("sin usuario: configúralo en la sesión".into()))?;

    let mut policy = AlgorithmPolicy::for_session(cfg);
    policy.prefer_known(&verifier.known_algorithms(&cfg.host, cfg.port));
    let (preferred, skipped): (Preferred, _) = policy.to_preferred();
    debug_assert!(skipped.is_empty(), "russh no conoce: {skipped:?}");
    let config = Arc::new(client::Config {
        preferred,
        keepalive_interval: cfg
            .keepalive_secs
            .filter(|s| *s > 0)
            .map(Duration::from_secs),
        ..Default::default()
    });

    let rejected = Arc::new(AtomicBool::new(false));
    let in_dialog = Arc::new(AtomicBool::new(false));
    let info = Arc::new(Info::default());
    info.require_strict
        .store(cfg.require_strict_kex, Ordering::SeqCst);
    info.agent
        .store(cfg.forward_agent && cfg!(unix), Ordering::SeqCst);
    let handler = Handler {
        verifier: verifier.clone(),
        host: cfg.host.clone(),
        port: cfg.port,
        rejected: rejected.clone(),
        in_dialog: in_dialog.clone(),
        info: info.clone(),
    };

    let timeout = timeout_of(cfg);
    // El plazo cubre el handshake, pero no mientras el usuario decide sobre la clave
    // de host (`in_dialog`): vencido el plazo con el diálogo abierto se espera sin límite.
    let hs = client::connect_stream(config, stream, handler);
    tokio::pin!(hs);
    let deadline = tokio::time::sleep(timeout);
    tokio::pin!(deadline);
    let mut expired = false;
    let res = loop {
        tokio::select! {
            r = &mut hs => break r,
            () = &mut deadline, if !expired => {
                if !in_dialog.load(Ordering::SeqCst) {
                    return Err(timed_out(cfg));
                }
                expired = true;
            }
        }
    };
    let strict_err = || {
        Error::Negotiation(
            "el servidor no anunció strict KEX (kex-strict-s-v00@openssh.com): \
             vulnerable a Terrapin; conexión abortada por require_strict_kex"
                .into(),
        )
    };
    let mut handle = match res {
        Ok(h) => h,
        Err(_) if info.strict_failed() => return Err(strict_err()),
        Err(e) => return Err(classify(e, rejected.load(Ordering::SeqCst))),
    };
    if info.strict_failed() {
        return Err(strict_err());
    }

    crate::auth::authenticate(
        &mut handle,
        cfg,
        &user,
        prompter.as_ref(),
        &rejected,
        warnings,
    )
    .await?;
    Ok((handle, info))
}

/// Órdenes de `ShellWriter` hacia la tarea escritora del canal.
pub(crate) enum ShellCmd {
    Data(Vec<u8>),
    Resize(u16, u16),
}

fn net(e: russh::Error) -> Error {
    classify(e, false)
}

fn io_err(what: &str, e: std::io::Error) -> Error {
    Error::Other(format!("{what}: {e}"))
}

fn signal_name(s: &russh::Sig) -> String {
    match s {
        russh::Sig::Custom(c) => c.clone(),
        other => format!("{other:?}"),
    }
}

pub(crate) async fn open_shell(
    handle: &Handle<Handler>,
    info: &Arc<Info>,
    term: &str,
    cols: u16,
    rows: u16,
) -> Result<(ShellWriter, ShellReader)> {
    let channel = handle.channel_open_session().await.map_err(net)?;
    if info.agent.load(Ordering::SeqCst) {
        channel.agent_forward(true).await.map_err(net)?;
    }
    channel
        .request_pty(true, term, cols.into(), rows.into(), 0, 0, &[])
        .await
        .map_err(net)?;
    channel.request_shell(true).await.map_err(net)?;

    // Escritura y lectura independientes: el lector drena siempre a russh, así que
    // ninguna de las dos puede dejar a la otra (ni a la conexión) bloqueada.
    let (mut read_half, write_half) = channel.split();
    let (cmd_tx, mut cmd_rx) = mpsc::channel::<ShellCmd>(WRITE_QUEUE);
    let (ev_tx, ev_rx) = mpsc::unbounded_channel::<ShellEvent>();
    let queued = Arc::new(AtomicUsize::new(0));

    tokio::spawn(async move {
        while let Some(cmd) = cmd_rx.recv().await {
            let r = match cmd {
                ShellCmd::Data(d) => write_half.data_bytes(d).await,
                ShellCmd::Resize(c, r) => write_half.window_change(c.into(), r.into(), 0, 0).await,
            };
            if r.is_err() {
                break; // al soltar `cmd_rx`, `ShellWriter` devuelve error de red
            }
        }
    });

    let info = info.clone();
    let pending = queued.clone();
    tokio::spawn(async move {
        let mut exited = false;
        let last = loop {
            let ev = match read_half.wait().await {
                // stderr: con PTY normalmente no aparece; se muestra igualmente.
                Some(ChannelMsg::Data { data } | ChannelMsg::ExtendedData { data, .. }) => {
                    if pending.fetch_add(data.len(), Ordering::SeqCst) + data.len()
                        > MAX_QUEUED_BYTES
                    {
                        break ShellEvent::Lost(
                            "la salida del servidor no se está leyendo (búfer desbordado)".into(),
                        );
                    }
                    ShellEvent::Data(data.to_vec())
                }
                Some(ChannelMsg::ExitStatus { exit_status }) => {
                    exited = true;
                    ShellEvent::ExitStatus(exit_status)
                }
                Some(ChannelMsg::ExitSignal {
                    signal_name: s,
                    core_dumped,
                    ..
                }) => {
                    exited = true;
                    ShellEvent::ExitSignal {
                        signal: signal_name(&s),
                        core_dumped,
                    }
                }
                Some(ChannelMsg::Close) | None if exited => break ShellEvent::Closed,
                Some(ChannelMsg::Close) => {
                    break ShellEvent::Lost(info.lost().unwrap_or_else(|| {
                        "el servidor cerró el canal sin estado de salida".into()
                    }));
                }
                None => {
                    break ShellEvent::Lost(
                        info.lost()
                            .unwrap_or_else(|| "conexión perdida con el servidor".into()),
                    );
                }
                Some(_) => continue,
            };
            if ev_tx.send(ev).is_err() {
                return;
            }
        };
        let _ = ev_tx.send(last);
    });

    Ok((
        ShellWriter { tx: cmd_tx },
        ShellReader { rx: ev_rx, queued },
    ))
}

pub(crate) async fn exec_stream<I, O, E>(
    handle: &Handle<Handler>,
    info: &Info,
    command: &str,
    mut stdin: I,
    mut stdout: O,
    mut stderr: E,
) -> Result<ExecStatus>
where
    I: AsyncRead + Unpin,
    O: AsyncWrite + Unpin,
    E: AsyncWrite + Unpin,
{
    let channel = handle.channel_open_session().await.map_err(net)?;
    channel.exec(true, command).await.map_err(net)?;
    let (mut read_half, write_half) = channel.split();
    let mut writer = write_half.make_writer();

    // stdin y salida avanzan a la vez; el EOF se envía al agotarse stdin.
    let sending = async {
        tokio::io::copy(&mut stdin, &mut writer)
            .await
            .map_err(|e| io_err("stdin", e))?;
        write_half.eof().await.map_err(net)
    };
    tokio::pin!(sending);
    let mut sent = false;
    let mut status = ExecStatus::default();
    loop {
        tokio::select! {
            r = &mut sending, if !sent => {
                sent = true;
                r?;
            }
            msg = read_half.wait() => match msg {
                Some(ChannelMsg::Data { data }) => {
                    stdout.write_all(&data).await.map_err(|e| io_err("stdout", e))?;
                }
                Some(ChannelMsg::ExtendedData { data, ext: 1 }) => {
                    stderr.write_all(&data).await.map_err(|e| io_err("stderr", e))?;
                }
                Some(ChannelMsg::ExitStatus { exit_status }) => status.exit_status = Some(exit_status),
                Some(ChannelMsg::ExitSignal { signal_name: s, .. }) => {
                    status.exit_signal = Some(signal_name(&s));
                }
                Some(ChannelMsg::Failure) => {
                    return Err(Error::Other("el servidor rechazó ejecutar la orden".into()));
                }
                Some(ChannelMsg::Close) | None => break,
                Some(_) => {}
            }
        }
    }
    stdout.flush().await.map_err(|e| io_err("stdout", e))?;
    stderr.flush().await.map_err(|e| io_err("stderr", e))?;
    if status.exit_status.is_none() && status.exit_signal.is_none() {
        return Err(Error::Network(format!(
            "el canal se cerró sin estado de salida ni señal: {}",
            info.lost().unwrap_or_else(|| "conexión perdida".into())
        )));
    }
    Ok(status)
}

pub(crate) enum Req<'a> {
    Subsystem(&'a str),
    Exec(&'a str),
}

/// Canal con subsistema o `exec` como flujo de bytes. El estado de salida queda en
/// `ChannelStream::exit_status`; stderr (datos extendidos) se descarta para no
/// mezclarlo con el protocolo que viaja por stdout.
pub(crate) async fn open_channel_stream(
    handle: &Handle<Handler>,
    req: Req<'_>,
) -> Result<crate::ChannelStream> {
    let channel = handle.channel_open_session().await.map_err(net)?;
    match req {
        Req::Subsystem(n) => channel.request_subsystem(true, n).await,
        Req::Exec(c) => channel.exec(true, c).await,
    }
    .map_err(net)?;
    let (mut read_half, write_half) = channel.split();
    // Esperar la respuesta a la petición antes de entregar el flujo.
    loop {
        match read_half.wait().await {
            Some(ChannelMsg::Success) => break,
            Some(ChannelMsg::Failure) => {
                return Err(Error::Other(
                    "el servidor rechazó la petición del canal".into(),
                ));
            }
            Some(ChannelMsg::Close) | None => {
                return Err(Error::Network("el canal se cerró antes de abrirse".into()));
            }
            Some(_) => {}
        }
    }

    let exit = Arc::new(Mutex::new(None));
    let (user_end, pump_end) = tokio::io::duplex(64 * 1024);
    let (mut from_user, mut to_user) = tokio::io::split(pump_end);
    let mut writer = write_half.make_writer();
    tokio::spawn(async move {
        let _ = tokio::io::copy(&mut from_user, &mut writer).await;
        let _ = write_half.eof().await;
    });
    let status = exit.clone();
    tokio::spawn(async move {
        loop {
            match read_half.wait().await {
                Some(ChannelMsg::Data { data }) => {
                    if to_user.write_all(&data).await.is_err() {
                        break;
                    }
                }
                Some(ChannelMsg::ExitStatus { exit_status }) => {
                    *status.lock().unwrap_or_else(PoisonError::into_inner) = Some(exit_status);
                }
                Some(ChannelMsg::Close) | None => break,
                Some(_) => {}
            }
        }
        // El estado de salida ya está guardado cuando el lector ve el EOF.
        let _ = to_user.shutdown().await;
    });
    Ok(crate::ChannelStream {
        inner: Box::pin(user_end),
        exit,
    })
}

#[cfg(test)]
mod jump_tests {
    use super::{Hop, parse_jumps};

    fn hop(user: Option<&str>, host: &str, port: u16) -> Hop {
        Hop {
            user: user.map(Into::into),
            host: host.into(),
            port,
        }
    }

    #[test]
    fn parsea_lista_de_saltos() {
        let got = parse_jumps("a@bastion:2200, interno ,[::1]:23,u@[fe80::1]").expect("válido");
        assert_eq!(
            got,
            [
                hop(Some("a"), "bastion", 2200),
                hop(None, "interno", 22),
                hop(None, "::1", 23),
                hop(Some("u"), "fe80::1", 22),
            ]
        );
    }

    #[test]
    fn rechaza_saltos_mal_formados() {
        for bad in ["", "@host", "host:xx", "host:99999", "[::1", ",a", "a,,b"] {
            assert!(parse_jumps(bad).is_err(), "{bad:?}");
        }
    }
}
