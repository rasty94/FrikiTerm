//! Conexión, canal de shell y exec sobre `russh::client`.

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
}

pub(crate) async fn connect(
    cfg: &SessionConfig,
    verifier: Arc<dyn HostKeyVerifier>,
    prompter: Arc<dyn AuthPrompter>,
) -> Result<Connected> {
    let user = cfg
        .user
        .clone()
        .or_else(default_user)
        .ok_or_else(|| Error::Other("sin usuario: configúralo en la sesión".into()))?;

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
    let handler = Handler {
        verifier,
        host: cfg.host.clone(),
        port: cfg.port,
        rejected: rejected.clone(),
        in_dialog: in_dialog.clone(),
        info: info.clone(),
    };

    let timeout = Duration::from_secs(cfg.connect_timeout_secs.max(1));
    let timed_out = || {
        Error::Network(format!(
            "tiempo de conexión agotado ({}s) con {}:{}",
            timeout.as_secs(),
            cfg.host,
            cfg.port
        ))
    };
    let stream = tokio::time::timeout(
        timeout,
        tokio::net::TcpStream::connect((cfg.host.as_str(), cfg.port)),
    )
    .await
    .map_err(|_| timed_out())?
    .map_err(|e| Error::Network(format!("{}:{}: {e}", cfg.host, cfg.port)))?;

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
                    return Err(timed_out());
                }
                expired = true;
            }
        }
    };
    let mut handle = res.map_err(|e| classify(e, rejected.load(Ordering::SeqCst)))?;

    crate::auth::authenticate(
        &mut handle,
        cfg,
        &user,
        prompter.as_ref(),
        &rejected,
        &mut warnings,
    )
    .await?;
    Ok(Connected {
        handle,
        info,
        warnings,
    })
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
