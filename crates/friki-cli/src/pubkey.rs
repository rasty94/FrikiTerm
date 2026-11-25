//! `friki pubkey-install`: como `ssh-copy-id`, por SFTP y sin duplicar la clave.

use std::path::Path;

use anyhow::{Context, bail};
use friki_cli::cli::{ConnArgs, PubkeyArgs};
use russh_sftp::client::SftpSession;
use russh_sftp::protocol::{FileAttributes, OpenFlags};
use tokio::io::AsyncWriteExt;

use crate::{UsageError, connect_target, home_dir};

const SSH_DIR: &str = ".ssh";
const AUTHORIZED: &str = ".ssh/authorized_keys";

fn mode(m: u32) -> FileAttributes {
    FileAttributes {
        permissions: Some(m),
        ..FileAttributes::empty()
    }
}

/// Primera línea no vacía del fichero, validada como clave pública.
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
    let text = std::fs::read_to_string(&path).with_context(|| path.display().to_string())?;
    let line = text.lines().map(str::trim).find(|l| !l.is_empty());
    match line {
        Some(l) if key_id(l).is_some() => Ok(l.to_string()),
        _ => Err(UsageError(format!("{} no es una clave pública", path.display())).into()),
    }
}

/// (tipo, base64) de una línea de `authorized_keys`; ignora opciones y comentarios.
fn key_id(line: &str) -> Option<(&str, &str)> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return None;
    }
    let mut f = line.split_whitespace();
    f.find(|t| t.starts_with("ssh-") || t.starts_with("ecdsa-") || t.starts_with("sk-"))
        .and_then(|ty| f.next().map(|b| (ty, b)))
}

/// Contenido nuevo de `authorized_keys`, o `None` si la clave ya está.
fn with_key(existing: &str, key: &str) -> Option<String> {
    let id = key_id(key)?;
    if existing.lines().any(|l| key_id(l) == Some(id)) {
        return None;
    }
    let sep = if existing.is_empty() || existing.ends_with('\n') {
        ""
    } else {
        "\n"
    };
    Some(format!("{existing}{sep}{key}\n"))
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
    };
    let conn = connect_target(&args.target, conn_args).await?;
    let res = async {
        let sftp = SftpSession::new(conn.open_subsystem("sftp").await?).await?;
        let added = add_key(&sftp, &key).await;
        let _ = sftp.close().await;
        added
    }
    .await;
    let _ = conn.disconnect().await;
    if res? {
        println!("clave instalada en {AUTHORIZED}");
    } else {
        println!("la clave ya estaba instalada");
    }
    Ok(0)
}

async fn add_key(sftp: &SftpSession, key: &str) -> anyhow::Result<bool> {
    if sftp.metadata(SSH_DIR).await.is_err() {
        sftp.create_dir(SSH_DIR).await?;
        sftp.set_metadata(SSH_DIR, mode(0o700)).await?;
    }
    // Si existe y no se puede leer, se aborta: reescribirlo perdería las claves ajenas.
    let existing = if sftp.try_exists(AUTHORIZED).await? {
        let bytes = sftp.read(AUTHORIZED).await?;
        String::from_utf8(bytes).or_else(|_| bail!("{AUTHORIZED} no es UTF-8; no se toca"))?
    } else {
        String::new()
    };
    let Some(new) = with_key(&existing, key) else {
        return Ok(false);
    };
    // Temporal 0600 + rename: un corte a medias nunca deja `authorized_keys` truncado.
    let tmp = format!("{AUTHORIZED}.friki-tmp");
    let mut f = sftp
        .open_with_flags_and_attributes(
            tmp.as_str(),
            OpenFlags::CREATE | OpenFlags::TRUNCATE | OpenFlags::WRITE,
            mode(0o600),
        )
        .await?;
    f.write_all(new.as_bytes()).await?;
    f.shutdown().await?;
    drop(f);
    sftp.set_metadata(tmp.as_str(), mode(0o600)).await?;
    let renamed = match sftp.posix_rename(tmp.as_str(), AUTHORIZED).await {
        Ok(true) => true,
        _ => sftp.rename(tmp.as_str(), AUTHORIZED).await.is_ok(),
    };
    if !renamed {
        // Sin sobrescritura por rename: se añade solo la línea nueva, sin truncar.
        let _ = sftp.remove_file(tmp.as_str()).await;
        let sep = if existing.is_empty() || existing.ends_with('\n') {
            ""
        } else {
            "\n"
        };
        let mut f = sftp
            .open_with_flags(
                AUTHORIZED,
                OpenFlags::WRITE | OpenFlags::APPEND | OpenFlags::CREATE,
            )
            .await?;
        f.write_all(format!("{sep}{key}\n").as_bytes()).await?;
        f.shutdown().await?;
        sftp.set_metadata(AUTHORIZED, mode(0o600)).await?;
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    const K: &str = "ssh-ed25519 AAAAC3Nza comentario";

    #[test]
    fn anade_a_vacio_y_sin_salto_final() {
        assert_eq!(with_key("", K).as_deref(), Some(&*format!("{K}\n")));
        assert_eq!(
            with_key("ssh-rsa BBB x", K).as_deref(),
            Some(&*format!("ssh-rsa BBB x\n{K}\n"))
        );
    }

    #[test]
    fn no_duplica_aunque_cambie_comentario_u_opciones() {
        assert!(with_key("ssh-ed25519 AAAAC3Nza otro\n", K).is_none());
        assert!(with_key("from=\"1.2.3.4\" ssh-ed25519 AAAAC3Nza\n", K).is_none());
        assert!(with_key("ssh-ed25519 AAAAC3NzaDISTINTA\n", K).is_some());
    }

    #[test]
    fn ignora_comentadas_y_vacias() {
        assert!(key_id("# ssh-ed25519 AAAAC3Nza x").is_none());
        assert!(key_id("   ").is_none());
        assert!(with_key("# ssh-ed25519 AAAAC3Nza x\n\n", K).is_some());
    }

    #[test]
    fn rechaza_lo_que_no_es_clave() {
        assert!(key_id("-----BEGIN OPENSSH PRIVATE KEY-----").is_none());
    }
}
