//! `friki pubkey-install`: como `ssh-copy-id`, por SFTP y sin duplicar la clave.

use std::path::Path;

use friki_cli::cli::{ConnArgs, PubkeyArgs};
use friki_session::pubkey::{self, AUTHORIZED};

use crate::{UsageError, connect_target, home_dir};

/// Clave indicada o la primera de `~/.ssh` (ed25519, ecdsa, rsa).
fn read_pubkey(path: Option<&Path>) -> anyhow::Result<String> {
    let path = match path {
        Some(p) => p.to_path_buf(),
        None => {
            let ssh = home_dir()?.join(".ssh");
            ["id_ed25519.pub", "id_ecdsa.pub", "id_rsa.pub"]
                .iter()
                .map(|n| ssh.join(n))
                .find(|p| p.is_file())
                .ok_or_else(|| UsageError("no hay clave pública; indica una con -i".into()))?
        }
    };
    pubkey::read_pubkey(&path)?
        .ok_or_else(|| UsageError(format!("{} no es una clave pública", path.display())).into())
}

pub async fn install(args: PubkeyArgs) -> anyhow::Result<i32> {
    let key = read_pubkey(args.pubkey.as_deref())?;
    let conn_args = ConnArgs {
        port: args.port,
        user: args.user,
        identity: args.auth_identity,
        legacy_algorithms: args.legacy_algorithms,
        jump: args.jump,
        fwd_local: Vec::new(),
        fwd_remote: Vec::new(),
        fwd_dynamic: Vec::new(),
        forward_agent: false,
        require_strict_kex: args.require_strict_kex,
        pkcs11: None,
        gssapi: false,
        gssapi_delegate: false,
    };
    let conn = connect_target(&args.target, conn_args).await?;
    let res = pubkey::install(&conn, &key).await;
    let _ = conn.disconnect().await;
    if res? {
        println!("clave instalada en {AUTHORIZED}");
    } else {
        println!("la clave ya estaba instalada");
    }
    Ok(0)
}
