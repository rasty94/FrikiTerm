//! `ls`, `get`, `put`, `cp`, `sync` y `ftp`: transferencias sin TUI sobre `friki-xfer`.

use std::collections::HashMap;
use std::io::IsTerminal;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use friki_cli::cli::{CpArgs, FtpAction, FtpConn, GetArgs, LsArgs, PutArgs, SyncArgs};
use friki_ssh::{Connection, Zeroizing};
use friki_xfer::cache::DirCache;
use friki_xfer::ftp::{self, FtpOptions};
use friki_xfer::local::LocalFs;
use friki_xfer::queue::{Direction, Job, Queue, QueueEvent, QueueOptions, Resume};
use friki_xfer::sync::{Action, Filters, SyncMode, SyncOptions, cache_key, plan, snapshot};
use friki_xfer::{Entry, Kind, RemoteFs, join, safe_name, scp, sftp};
use serde_json::json;
use tokio::sync::mpsc::UnboundedReceiver;

use crate::net::split_host_port;
use crate::{CodedError, EXIT_OTHER, UsageError, connect_target, failure_code, sanitize_remote};
use friki_xfer::ErrorKind;

type Fs = Arc<dyn RemoteFs>;

fn usage(msg: impl Into<String>) -> anyhow::Error {
    UsageError(msg.into()).into()
}

/// Separa `destino:ruta` (como scp: los `:` tras un `/` o dentro de `[..]` no cuentan).
/// Una letra y `:` seguidos de `\`, `/` o el fin (`C:\x`, `c:/x`, `C:`) es una unidad de
/// Windows, es decir una ruta local, en cualquier plataforma; un host de una letra se
/// escribe con usuario (`u@c:ruta`).
pub fn split_remote(spec: &str) -> Option<(&str, &str)> {
    let b = spec.as_bytes();
    if b.len() >= 2
        && b[0].is_ascii_alphabetic()
        && b[1] == b':'
        && matches!(b.get(2), None | Some(b'\\' | b'/'))
    {
        return None;
    }
    let start = spec.find('@').map_or(0, |i| i + 1);
    let host_end = if spec[start..].starts_with('[') {
        start + spec[start..].find(']')? + 1
    } else {
        start
    };
    let colon = host_end + spec[host_end..].find(':')?;
    (!spec[..colon].contains('/') && colon > 0).then(|| (&spec[..colon], &spec[colon + 1..]))
}

fn need_remote(spec: &str) -> anyhow::Result<(&str, &str)> {
    split_remote(spec).ok_or_else(|| usage(format!("se esperaba destino:ruta, no `{spec}`")))
}

/// Ruta remota vacía = directorio de entrada.
fn remote_path(p: &str) -> &str {
    if p.is_empty() { "." } else { p }
}

fn base_name(path: &str) -> anyhow::Result<&str> {
    let name = path.trim_end_matches('/').rsplit('/').next().unwrap_or("");
    if safe_name(name) {
        Ok(name)
    } else {
        Err(usage(format!("no se puede deducir un nombre de `{path}`")))
    }
}

async fn open_sftp(conn: &Connection) -> anyhow::Result<Fs> {
    Ok(Arc::from(sftp::open(conn).await?))
}

// ------------------------------------------------------------------ rutas locales

/// Ruta absoluta y normalizada léxicamente (sin tocar el disco).
fn absolute(p: &Path) -> anyhow::Result<PathBuf> {
    let full = std::env::current_dir()?.join(p);
    let mut out = PathBuf::new();
    for c in full.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            c => out.push(c.as_os_str()),
        }
    }
    Ok(out)
}

/// `LocalFs` desde la raíz del sistema de ficheros, para aceptar rutas absolutas.
fn local_root() -> anyhow::Result<PathBuf> {
    let cwd = std::env::current_dir()?;
    Ok(cwd.ancestors().last().unwrap_or(&cwd).to_path_buf())
}

/// Ruta de `p` relativa a la raíz, con `/` como separador (la que entiende `LocalFs`).
fn local_rel(p: &Path) -> anyhow::Result<String> {
    let abs = absolute(p)?;
    let rel = abs.strip_prefix(local_root()?)?;
    Ok(rel
        .components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/"))
}

// ------------------------------------------------------------------ cola y progreso

fn bar(file: &str, done: u64, total: Option<u64>) -> String {
    let file = sanitize_remote(file);
    match total.filter(|t| *t > 0) {
        Some(t) => {
            let pct = (done.min(t) * 100 / t) as usize;
            let n = pct / 5;
            format!("[{}{}] {pct:3}% {file}", "#".repeat(n), ".".repeat(20 - n))
        }
        None => format!("{done} bytes {file}"),
    }
}

/// Pinta los eventos de la cola (barra en stderr si es un TTY, JSON si se pide).
/// Devuelve el primer fallo.
async fn progress(
    mut rx: UnboundedReceiver<QueueEvent>,
    json: bool,
) -> Option<(String, ErrorKind)> {
    let tty = !json && std::io::stderr().is_terminal();
    let mut totals: HashMap<u64, Option<u64>> = HashMap::new();
    let mut first = None;
    while let Some(ev) = rx.recv().await {
        match ev {
            QueueEvent::Started { id, file, total } => {
                if json {
                    println!("{}", json!({"event":"started","file":file,"total":total}));
                }
                totals.insert(id, total);
            }
            QueueEvent::Progress { id, file, done } => {
                if json {
                    println!("{}", json!({"event":"progress","file":file,"done":done}));
                } else if tty {
                    let total = totals.get(&id).copied().flatten();
                    eprint!("\r\x1b[K{}", bar(&file, done, total));
                }
            }
            QueueEvent::Retrying { attempt, error, .. } => {
                if json {
                    println!(
                        "{}",
                        json!({"event":"retrying","attempt":attempt,"error":error})
                    );
                } else {
                    eprintln!(
                        "\r\x1b[Kfriki: reintento {attempt}: {}",
                        sanitize_remote(&error)
                    );
                }
            }
            QueueEvent::Finished { id, bytes } => {
                totals.remove(&id);
                if json {
                    println!("{}", json!({"event":"finished","bytes":bytes}));
                } else if tty {
                    eprintln!("\r\x1b[K{bytes} bytes transferidos");
                }
            }
            QueueEvent::Failed { id, error, kind } => {
                totals.remove(&id);
                if json {
                    println!("{}", json!({"event":"failed","error":error}));
                } else {
                    eprintln!("\r\x1b[Kfriki: error: {}", sanitize_remote(&error));
                }
                first.get_or_insert((error, kind));
            }
            QueueEvent::Cancelled { .. } => {}
        }
    }
    first
}

async fn run_jobs(local: Fs, remote: Fs, jobs: Vec<Job>, json: bool) -> anyhow::Result<()> {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let queue = Queue::new(local, remote, QueueOptions::default(), tx);
    let printer = tokio::spawn(progress(rx, json));
    for job in jobs {
        queue.push(job);
    }
    let (ok, failed) = queue.wait().await;
    drop(queue);
    let first = printer.await.unwrap_or(None);
    if json {
        println!("{}", json!({"event":"done","ok":ok,"failed":failed}));
    }
    if failed > 0 {
        let (msg, kind) =
            first.unwrap_or_else(|| ("transferencia fallida".into(), ErrorKind::Other));
        return Err(CodedError(
            failure_code(kind),
            format!(
                "{failed} transferencia(s) fallida(s): {}",
                sanitize_remote(&msg)
            ),
        )
        .into());
    }
    Ok(())
}

fn job(direction: Direction, src: String, dst: String, recursive: bool) -> Job {
    Job {
        direction,
        src,
        dst,
        recursive,
        // ponytail: sin reanudación; un destino parcial se reescribe entero.
        resume: Resume::Never,
        preserve_mtime: true,
    }
}

// ------------------------------------------------------------------ get / put

async fn download_fs(
    remote: Fs,
    rpaths: &[&str],
    local: Option<&Path>,
    recursive: bool,
    json: bool,
) -> anyhow::Result<()> {
    let into_dir = local.is_none_or(Path::is_dir);
    if rpaths.len() > 1 && !into_dir {
        return Err(usage(format!(
            "`{}` debe ser un directorio",
            local.map_or_else(String::new, |l| l.display().to_string())
        )));
    }
    let mut jobs = Vec::new();
    for rpath in rpaths {
        let rpath = remote_path(rpath);
        let dest = match local {
            Some(l) if l.is_dir() => l.join(base_name(rpath)?),
            Some(l) => l.to_path_buf(),
            None => PathBuf::from(base_name(rpath)?),
        };
        jobs.push(job(
            Direction::Download,
            rpath.to_string(),
            local_rel(&dest)?,
            recursive,
        ));
    }
    let fs: Fs = Arc::new(LocalFs::new(local_root()?));
    run_jobs(fs, remote, jobs, json).await
}

async fn upload_fs(
    remote: Fs,
    locals: &[PathBuf],
    rpath: &str,
    recursive: bool,
    json: bool,
) -> anyhow::Result<()> {
    let rpath = remote_path(rpath);
    let is_dir = rpath.ends_with('/')
        || remote
            .stat(rpath)
            .await?
            .is_some_and(|e| e.kind == Kind::Dir);
    if locals.len() > 1 && !is_dir {
        return Err(usage(format!("`{rpath}` debe ser un directorio remoto")));
    }
    let mut jobs = Vec::new();
    for l in locals {
        let dst = if is_dir {
            let name = l
                .file_name()
                .and_then(|n| n.to_str())
                .filter(|n| safe_name(n))
                .ok_or_else(|| usage(format!("no se puede subir `{}`", l.display())))?;
            join(rpath, name)
        } else {
            rpath.to_string()
        };
        jobs.push(job(Direction::Upload, local_rel(l)?, dst, recursive));
    }
    let fs: Fs = Arc::new(LocalFs::new(local_root()?));
    run_jobs(fs, remote, jobs, json).await
}

/// SCP: sin cola ni progreso por fichero; informa del total.
async fn scp_up(
    conn: &Connection,
    locals: &[PathBuf],
    rpath: &str,
    recursive: bool,
    json: bool,
) -> anyhow::Result<()> {
    let mut total = 0;
    for l in locals {
        total += scp::upload(conn, l, remote_path(rpath), recursive).await?;
    }
    scp_done(total, json);
    Ok(())
}

async fn scp_down(
    conn: &Connection,
    rpaths: &[&str],
    local: Option<&Path>,
    recursive: bool,
    json: bool,
) -> anyhow::Result<()> {
    let dir = match local {
        None => PathBuf::from("."),
        Some(l) if l.is_dir() => l.to_path_buf(),
        Some(_) => return Err(usage("con --scp el destino local debe ser un directorio")),
    };
    let mut total = 0;
    for r in rpaths {
        total += scp::download(conn, remote_path(r), &dir, recursive).await?;
    }
    scp_done(total, json);
    Ok(())
}

fn scp_done(bytes: u64, json: bool) {
    if json {
        println!(
            "{}",
            json!({"event":"done","ok":1,"failed":0,"bytes":bytes})
        );
    } else if std::io::stderr().is_terminal() {
        eprintln!("{bytes} bytes transferidos");
    }
}

async fn finish(conn: Connection, res: anyhow::Result<()>) -> anyhow::Result<i32> {
    let _ = conn.disconnect().await;
    res.map(|()| 0)
}

pub async fn get(args: GetArgs) -> anyhow::Result<i32> {
    let (target, rpath) = need_remote(&args.remote)?;
    let conn = connect_target(target, args.conn).await?;
    let res = async {
        let fs = open_sftp(&conn).await?;
        download_fs(
            fs,
            &[rpath],
            args.local.as_deref(),
            args.recursive,
            args.json,
        )
        .await
    }
    .await;
    finish(conn, res).await
}

pub async fn put(args: PutArgs) -> anyhow::Result<i32> {
    let Some((remote, locals)) = args.paths.split_last() else {
        return Err(usage("faltan argumentos"));
    };
    let (target, rpath) = need_remote(remote)?;
    let locals: Vec<PathBuf> = locals.iter().map(PathBuf::from).collect();
    let conn = connect_target(target, args.conn).await?;
    let res = async {
        let fs = open_sftp(&conn).await?;
        upload_fs(fs, &locals, rpath, args.recursive, args.json).await
    }
    .await;
    finish(conn, res).await
}

pub async fn cp(args: CpArgs) -> anyhow::Result<i32> {
    let Some((dst, srcs)) = args.paths.split_last() else {
        return Err(usage("faltan argumentos"));
    };
    let (rec, json, scp) = (args.recursive, args.json, args.scp);
    if let Some((target, rpath)) = split_remote(dst) {
        if srcs.iter().any(|s| split_remote(s).is_some()) {
            return Err(usage("solo un lado puede ser remoto"));
        }
        let locals: Vec<PathBuf> = srcs.iter().map(PathBuf::from).collect();
        let conn = connect_target(target, args.conn).await?;
        let res = async {
            if scp {
                scp_up(&conn, &locals, rpath, rec, json).await
            } else {
                upload_fs(open_sftp(&conn).await?, &locals, rpath, rec, json).await
            }
        }
        .await;
        return finish(conn, res).await;
    }
    // Varios orígenes remotos: todos en el mismo destino (una sola conexión).
    let parsed = srcs
        .iter()
        .map(|s| need_remote(s))
        .collect::<anyhow::Result<Vec<_>>>()?;
    let target = parsed[0].0;
    if parsed.iter().any(|(t, _)| *t != target) {
        return Err(usage(
            "los orígenes remotos deben estar en el mismo destino",
        ));
    }
    let rpaths: Vec<&str> = parsed.iter().map(|(_, p)| *p).collect();
    let local = Some(PathBuf::from(dst));
    let conn = connect_target(target, args.conn).await?;
    let res = async {
        if scp {
            scp_down(&conn, &rpaths, local.as_deref(), rec, json).await
        } else {
            download_fs(
                open_sftp(&conn).await?,
                &rpaths,
                local.as_deref(),
                rec,
                json,
            )
            .await
        }
    }
    .await;
    finish(conn, res).await
}

// ------------------------------------------------------------------ ls

fn kind_name(k: Kind) -> &'static str {
    match k {
        Kind::File => "file",
        Kind::Dir => "dir",
        Kind::Symlink => "symlink",
        Kind::Other => "other",
    }
}

fn print_entries(mut entries: Vec<Entry>, json: bool) {
    entries.sort_by(|a, b| a.name.cmp(&b.name));
    if json {
        let list: Vec<_> = entries
            .iter()
            .map(|e| {
                json!({"name":e.name,"path":e.path,"kind":kind_name(e.kind),
                       "size":e.size,"mtime":e.mtime,"mode":e.mode})
            })
            .collect();
        println!("{}", json!(list));
        return;
    }
    for e in entries {
        let k = match e.kind {
            Kind::Dir => 'd',
            Kind::Symlink => 'l',
            Kind::File => '-',
            Kind::Other => '?',
        };
        let mode = e.mode.map_or("----".to_string(), |m| format!("{m:04o}"));
        println!("{k} {mode} {:>12} {}", e.size, sanitize_remote(&e.name));
    }
}

pub async fn ls(args: LsArgs) -> anyhow::Result<i32> {
    let (target, rpath) = need_remote(&args.remote)?;
    let conn = connect_target(target, args.conn).await?;
    let res = async {
        let fs = open_sftp(&conn).await?;
        print_entries(fs.list(remote_path(rpath)).await?, args.json);
        Ok(())
    }
    .await;
    finish(conn, res).await
}

// ------------------------------------------------------------------ sync

fn action_parts(a: &Action) -> (&'static str, &str) {
    match a {
        Action::Upload { path } => ("upload", path),
        Action::Download { path } => ("download", path),
        Action::MkdirRemote { path } => ("mkdir-remote", path),
        Action::MkdirLocal { path } => ("mkdir-local", path),
        Action::DeleteRemote { path } => ("delete-remote", path),
        Action::DeleteLocal { path } => ("delete-local", path),
        Action::Conflict { path } => ("conflict", path),
    }
}

/// `mkdir -p`: crea cada componente que falte.
async fn mkdir_p(fs: &dyn RemoteFs, path: &str) -> friki_xfer::Result<()> {
    let mut cur = String::new();
    for part in path.split('/').filter(|p| !p.is_empty() && *p != ".") {
        cur = if cur.is_empty() && !path.starts_with('/') {
            part.to_string()
        } else {
            format!("{cur}/{part}")
        };
        if fs.stat(&cur).await?.is_none() {
            fs.mkdir(&cur).await?;
        }
    }
    Ok(())
}

async fn remove(fs: &dyn RemoteFs, path: &str) -> friki_xfer::Result<()> {
    match fs.stat(path).await? {
        None => Ok(()),
        Some(e) if e.kind == Kind::Dir => fs.remove_dir(path).await,
        Some(_) => fs.remove_file(path).await,
    }
}

const MAX_DELETES_SIN_CONFIRMAR: usize = 20;

#[derive(Debug, PartialEq)]
enum Gate {
    Go,
    Ask,
    Refuse,
}

/// Más de 20 borrados exigen `--yes` o confirmación en terminal.
fn delete_gate(n: usize, yes: bool, tty: bool) -> Gate {
    match (n > MAX_DELETES_SIN_CONFIRMAR && !yes, tty) {
        (false, _) => Gate::Go,
        (true, true) => Gate::Ask,
        (true, false) => Gate::Refuse,
    }
}

async fn sync_run(args: SyncArgs, remote: Fs, rroot: &str) -> anyhow::Result<()> {
    let mode = if args.both {
        SyncMode::Both
    } else if args.down {
        SyncMode::Download
    } else {
        SyncMode::Upload
    };
    let dir = absolute(&args.local)?;
    if !dir.is_dir() {
        if mode == SyncMode::Upload {
            return Err(friki_xfer::Error::NotFound(dir.display().to_string()).into());
        }
        if !args.dry_run {
            std::fs::create_dir_all(&dir)?;
        }
    }
    // Raíz de origen remota inexistente: error claro antes de planificar (y de borrar nada).
    if mode == SyncMode::Download && remote.stat(rroot).await?.is_none() {
        return Err(friki_xfer::Error::NotFound(format!("origen remoto {rroot}")).into());
    }
    let local: Fs = Arc::new(LocalFs::new(dir));
    let opts = SyncOptions {
        mode,
        delete: args.delete,
        filters: Filters {
            include: args.include,
            exclude: args.exclude,
        },
        mtime_tolerance: 1,
        ..Default::default()
    };
    // Solo `--both` usa caché: sin ella no distingue «borrado» de «nuevo en el otro lado».
    let cache = if mode == SyncMode::Both {
        let path = friki_core::paths::config_dir().join("sync-cache.db");
        if let Some(d) = path.parent() {
            std::fs::create_dir_all(d)?;
        }
        Some(DirCache::open(&path)?)
    } else {
        None
    };
    let actions = plan(&*local, "", &*remote, rroot, &opts, cache.as_ref()).await?;
    if args.dry_run {
        for a in &actions {
            let (name, path) = action_parts(a);
            if args.json {
                println!("{}", json!({"action":name,"path":path}));
            } else {
                println!("{name}\t{}", sanitize_remote(path));
            }
        }
        return Ok(());
    }
    let n_del = actions
        .iter()
        .filter(|a| matches!(a, Action::DeleteRemote { .. } | Action::DeleteLocal { .. }))
        .count();
    match delete_gate(n_del, args.yes, std::io::stdin().is_terminal()) {
        Gate::Go => {}
        Gate::Refuse => {
            return Err(usage(format!(
                "el plan borra {n_del} elementos; confirma con --yes"
            )));
        }
        Gate::Ask => {
            let q = format!("el plan borra {n_del} elementos. ¿Continuar? [s/N] ");
            let ok = crate::ask(&q, true).await;
            if !ok.is_some_and(|r| matches!(r.trim(), "s" | "S" | "y" | "Y")) {
                return Err(usage("cancelado por el usuario"));
            }
        }
    }
    if mode != SyncMode::Download {
        mkdir_p(&*remote, rroot).await?;
    }
    let (mut jobs, mut deletes, mut conflicts) = (Vec::new(), Vec::new(), Vec::new());
    for a in &actions {
        match a {
            Action::MkdirRemote { path } => remote.mkdir(&join(rroot, path)).await?,
            Action::MkdirLocal { path } => local.mkdir(path).await?,
            Action::Upload { path } => {
                jobs.push(job(
                    Direction::Upload,
                    path.clone(),
                    join(rroot, path),
                    false,
                ));
            }
            Action::Download { path } => {
                jobs.push(job(
                    Direction::Download,
                    join(rroot, path),
                    path.clone(),
                    false,
                ));
            }
            Action::DeleteRemote { .. } | Action::DeleteLocal { .. } => deletes.push(a),
            Action::Conflict { path } => conflicts.push(path.clone()),
        }
    }
    run_jobs(local.clone(), remote.clone(), jobs, args.json).await?;
    for a in deletes {
        match a {
            Action::DeleteRemote { path } => remove(&*remote, &join(rroot, path)).await?,
            Action::DeleteLocal { path } => remove(&*local, path).await?,
            _ => {}
        }
        let (name, path) = action_parts(a);
        if args.json {
            println!("{}", json!({"event":"deleted","action":name,"path":path}));
        } else {
            eprintln!("{name} {}", sanitize_remote(path));
        }
    }
    if !conflicts.is_empty() {
        return Err(CodedError(
            EXIT_OTHER,
            format!("conflictos sin resolver: {}", conflicts.join(", ")),
        )
        .into());
    }
    // Solo tras una ejecución completa: el estado local pasa a ser la referencia.
    if let Some(c) = cache {
        let key = cache_key(&*local, "", &*remote, rroot);
        c.store(&key, &snapshot(&*local, "").await?)?;
    }
    Ok(())
}

pub async fn sync(args: SyncArgs) -> anyhow::Result<i32> {
    let spec = args.remote.clone();
    let (target, rpath) = need_remote(&spec)?;
    let conn = connect_target(target, args.conn.clone()).await?;
    let res = async {
        let fs = open_sftp(&conn).await?;
        sync_run(args, fs, remote_path(rpath)).await
    }
    .await;
    finish(conn, res).await
}

// ------------------------------------------------------------------ ftp

/// FTP sin TLS: aviso siempre; un usuario no anónimo exige `--insecure-plaintext`.
fn check_plaintext(c: &FtpConn) -> anyhow::Result<()> {
    if c.tls {
        return Ok(());
    }
    eprintln!("friki: aviso: FTP sin cifrar; usa --tls si el servidor lo admite");
    if c.user != "anonymous" && !c.insecure_plaintext {
        return Err(usage(
            "credenciales en claro: añade --tls o --insecure-plaintext",
        ));
    }
    Ok(())
}

async fn ftp_open(c: &FtpConn) -> anyhow::Result<Fs> {
    check_plaintext(c)?;
    let (host, port) =
        split_host_port(&c.host).ok_or_else(|| usage(format!("destino no válido: {}", c.host)))?;
    let password = match std::env::var("FRIKI_FTP_PASSWORD") {
        Ok(p) => Zeroizing::new(p),
        Err(_) if c.user == "anonymous" => Zeroizing::new("anonymous@".to_string()),
        Err(_) if std::io::stdin().is_terminal() => {
            crate::ask(&format!("{}@{} password: ", c.user, c.host), false)
                .await
                .ok_or_else(|| usage("contraseña FTP no introducida"))?
        }
        Err(_) => {
            return Err(usage("define FRIKI_FTP_PASSWORD o usa una terminal"));
        }
    };
    let opts = FtpOptions {
        host,
        port: port.unwrap_or(21),
        user: c.user.clone(),
        tls: c.tls,
        passive: true,
    };
    Ok(Arc::from(ftp::open(&opts, &password).await?))
}

pub async fn ftp(action: FtpAction) -> anyhow::Result<i32> {
    match action {
        FtpAction::Ls { ftp, path, json } => {
            let fs = ftp_open(&ftp).await?;
            print_entries(fs.list(path.as_deref().unwrap_or(".")).await?, json);
        }
        FtpAction::Get {
            ftp,
            remote,
            local,
            recursive,
            json,
        } => {
            let fs = ftp_open(&ftp).await?;
            download_fs(fs, &[&remote], local.as_deref(), recursive, json).await?;
        }
        FtpAction::Put {
            ftp,
            local,
            remote,
            recursive,
            json,
        } => {
            let fs = ftp_open(&ftp).await?;
            let rpath = remote.as_deref().unwrap_or(".");
            upload_fs(fs, &[local], rpath, recursive, json).await?;
        }
    }
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_remote_casos() {
        assert_eq!(split_remote("host:/tmp/x"), Some(("host", "/tmp/x")));
        assert_eq!(split_remote("ana@host:"), Some(("ana@host", "")));
        assert_eq!(split_remote("ana@[::1]:ruta"), Some(("ana@[::1]", "ruta")));
        assert_eq!(split_remote("[::1]:r:s"), Some(("[::1]", "r:s")));
        assert_eq!(split_remote("./a:b"), None);
        assert_eq!(split_remote("/tmp/a:b"), None);
        assert_eq!(split_remote("fichero"), None);
        assert_eq!(split_remote(":x"), None);
        assert_eq!(split_remote("C:\\a"), None);
        assert_eq!(split_remote("c:/a"), None);
        assert_eq!(split_remote("C:"), None);
        assert_eq!(split_remote("u@c:ruta"), Some(("u@c", "ruta")));
        assert_eq!(split_remote("c:ruta"), Some(("c", "ruta")));
    }

    #[cfg(windows)]
    #[test]
    fn rutas_locales_se_normalizan() {
        let abs = absolute(Path::new(r"C:\a\b\..\c")).expect("absoluta");
        assert_eq!(abs, PathBuf::from(r"C:\a\c"));
        // local_rel depende de la unidad del directorio actual.
        if abs.starts_with(local_root().expect("raíz")) {
            assert_eq!(local_rel(&abs).ok().as_deref(), Some("a/c"));
        }
    }

    #[cfg(unix)]
    #[test]
    fn rutas_locales_se_normalizan() {
        let abs = absolute(Path::new("/a/b/../c/./d")).expect("absoluta");
        assert_eq!(abs, PathBuf::from("/a/c/d"));
        assert_eq!(
            local_rel(Path::new("/a/b/../c")).ok().as_deref(),
            Some("a/c")
        );
    }

    #[test]
    fn ftp_en_claro_exige_flag() {
        let mut c = FtpConn {
            host: "h".into(),
            user: "ana".into(),
            tls: false,
            insecure_plaintext: false,
        };
        assert!(check_plaintext(&c).is_err());
        c.insecure_plaintext = true;
        assert!(check_plaintext(&c).is_ok());
        c.insecure_plaintext = false;
        c.tls = true;
        assert!(check_plaintext(&c).is_ok());
        c.tls = false;
        c.user = "anonymous".into();
        assert!(check_plaintext(&c).is_ok());
    }

    #[test]
    fn delete_gate_casos() {
        assert_eq!(delete_gate(20, false, false), Gate::Go);
        assert_eq!(delete_gate(21, true, false), Gate::Go);
        assert_eq!(delete_gate(21, false, true), Gate::Ask);
        assert_eq!(delete_gate(21, false, false), Gate::Refuse);
    }

    #[test]
    fn nombre_base() {
        assert_eq!(base_name("/x/y/").ok(), Some("y"));
        assert!(base_name(".").is_err());
        assert!(base_name("/").is_err());
    }

    #[test]
    fn xfer_errores_dan_codigos() {
        assert_eq!(failure_code(ErrorKind::Network), crate::EXIT_NETWORK);
        assert_eq!(failure_code(ErrorKind::PermissionDenied), EXIT_OTHER);
        assert_eq!(failure_code(ErrorKind::NotFound), EXIT_OTHER);
    }
}
