//! Conexión, canal de shell y exec sobre `russh::client`.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use friki_core::config::SessionConfig;
use friki_core::known_hosts::HostKey;
use russh::client::{self, Handle};
use russh::keys::PublicKeyOrCertificate;
use russh::{ChannelMsg, Preferred};
use tokio::sync::{mpsc, oneshot};

use crate::errors::classify;
use crate::{
    AlgorithmPolicy, AuthPrompter, Error, ExecOutput, HostKeyDecision, HostKeyVerifier, Result,
    ShellEvent, ShellReader, ShellWriter,
};

pub(crate) struct Handler {
    verifier: Arc<dyn HostKeyVerifier>,
    host: String,
    port: u16,
    rejected: Arc<AtomicBool>,
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
        let ok = self.verifier.verify(&self.host, self.port, &hk).await != HostKeyDecision::Reject;
        if !ok {
            self.rejected.store(true, Ordering::SeqCst);
        }
        Ok(ok)
    }
}

fn default_user() -> Option<String> {
    ["USER", "USERNAME", "LOGNAME"]
        .iter()
        .find_map(|v| std::env::var(v).ok())
        .filter(|u| !u.is_empty())
}

pub(crate) async fn connect(
    cfg: &SessionConfig,
    verifier: Arc<dyn HostKeyVerifier>,
    prompter: Arc<dyn AuthPrompter>,
) -> Result<Handle<Handler>> {
    let user = cfg
        .user
        .clone()
        .or_else(default_user)
        .ok_or_else(|| Error::Other("sin usuario: configúralo en la sesión".into()))?;

    let (preferred, skipped): (Preferred, _) = AlgorithmPolicy::for_session(cfg).to_preferred();
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
    let handler = Handler {
        verifier,
        host: cfg.host.clone(),
        port: cfg.port,
        rejected: rejected.clone(),
    };

    let timeout = Duration::from_secs(cfg.connect_timeout_secs.max(1));
    let connecting = client::connect(config, (cfg.host.as_str(), cfg.port), handler);
    let mut handle = match tokio::time::timeout(timeout, connecting).await {
        Err(_) => {
            return Err(Error::Network(format!(
                "tiempo de conexión agotado ({}s) con {}:{}",
                timeout.as_secs(),
                cfg.host,
                cfg.port
            )));
        }
        Ok(r) => r.map_err(|e| classify(e, rejected.load(Ordering::SeqCst)))?,
    };

    crate::auth::authenticate(&mut handle, cfg, &user, prompter.as_ref(), &rejected).await?;
    Ok(handle)
}

/// Órdenes de `ShellWriter` hacia la tarea dueña del canal.
pub(crate) enum ShellCmd {
    Data(Vec<u8>, oneshot::Sender<Result<()>>),
    Resize(u16, u16, oneshot::Sender<Result<()>>),
}

fn net(e: russh::Error) -> Error {
    classify(e, false)
}

pub(crate) async fn open_shell(
    handle: &Handle<Handler>,
    term: &str,
    cols: u16,
    rows: u16,
) -> Result<(ShellWriter, ShellReader)> {
    let mut channel = handle.channel_open_session().await.map_err(net)?;
    channel
        .request_pty(true, term, cols.into(), rows.into(), 0, 0, &[])
        .await
        .map_err(net)?;
    channel.request_shell(true).await.map_err(net)?;

    let (cmd_tx, mut cmd_rx) = mpsc::channel::<ShellCmd>(64);
    let (ev_tx, ev_rx) = mpsc::channel::<ShellEvent>(256);

    tokio::spawn(async move {
        let mut cmds_open = true;
        loop {
            tokio::select! {
                cmd = cmd_rx.recv(), if cmds_open => match cmd {
                    None => cmds_open = false,
                    Some(ShellCmd::Data(d, ack)) => {
                        let _ = ack.send(channel.data(&d[..]).await.map_err(net));
                    }
                    Some(ShellCmd::Resize(c, r, ack)) => {
                        let _ = ack.send(channel.window_change(c.into(), r.into(), 0, 0).await.map_err(net));
                    }
                },
                msg = channel.wait() => {
                    let ev = match msg {
                        Some(ChannelMsg::Data { data }) => ShellEvent::Data(data.to_vec()),
                        // stderr: con PTY normalmente no aparece; se muestra igualmente.
                        Some(ChannelMsg::ExtendedData { data, .. }) => ShellEvent::Data(data.to_vec()),
                        Some(ChannelMsg::ExitStatus { exit_status }) => ShellEvent::ExitStatus(exit_status),
                        Some(ChannelMsg::Close) | None => break,
                        Some(_) => continue,
                    };
                    if ev_tx.send(ev).await.is_err() {
                        break;
                    }
                }
            }
        }
        let _ = ev_tx.send(ShellEvent::Closed).await;
    });

    Ok((ShellWriter { tx: cmd_tx }, ShellReader { rx: ev_rx }))
}

pub(crate) async fn exec(handle: &Handle<Handler>, command: &str) -> Result<ExecOutput> {
    let mut channel = handle.channel_open_session().await.map_err(net)?;
    channel.exec(true, command).await.map_err(net)?;
    let mut out = ExecOutput {
        stdout: Vec::new(),
        stderr: Vec::new(),
        exit_status: None,
    };
    while let Some(msg) = channel.wait().await {
        match msg {
            ChannelMsg::Data { data } => out.stdout.extend_from_slice(&data),
            ChannelMsg::ExtendedData { data, ext: 1 } => out.stderr.extend_from_slice(&data),
            ChannelMsg::ExitStatus { exit_status } => out.exit_status = Some(exit_status),
            ChannelMsg::Close => break,
            _ => {}
        }
    }
    Ok(out)
}
