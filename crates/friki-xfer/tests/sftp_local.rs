//! SFTP contra el `sftp-server` del sistema por stdin/stdout (sin red ni sshd).
//! Si el binario no existe la prueba lo dice y termina.

#![cfg(unix)]

use std::path::Path;
use std::process::Stdio;

use friki_xfer::{Error, Kind, RemoteFs, sftp};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::{Child, Command};

fn server_path() -> Option<&'static str> {
    [
        "/usr/libexec/sftp-server",
        "/usr/lib/openssh/sftp-server",
        "/usr/lib/ssh/sftp-server",
        "/usr/libexec/openssh/sftp-server",
    ]
    .into_iter()
    .find(|p| Path::new(p).exists())
}

async fn start() -> Option<(Box<dyn RemoteFs>, Child)> {
    let Some(bin) = server_path() else {
        eprintln!("SALTADA (no ejecutada): no hay sftp-server en el host");
        return None;
    };
    let mut child = Command::new(bin)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .expect("lanzar sftp-server");
    let io = tokio::io::join(
        child.stdout.take().expect("stdout"),
        child.stdin.take().expect("stdin"),
    );
    let fs = sftp::open_stream(io, "sftp://test@local")
        .await
        .expect("sesión SFTP");
    Some((fs, child))
}

async fn put(fs: &dyn RemoteFs, path: &str, offset: u64, data: &[u8]) {
    let mut w = fs.open_write(path, offset).await.expect("open_write");
    w.write_all(data).await.expect("write");
    w.shutdown().await.expect("shutdown");
}

async fn get(fs: &dyn RemoteFs, path: &str, offset: u64) -> Vec<u8> {
    let mut r = fs.open_read(path, offset).await.expect("open_read");
    let mut v = Vec::new();
    r.read_to_end(&mut v).await.expect("read");
    v
}

#[tokio::test]
async fn sftp_ciclo_completo() {
    let Some((fs, _child)) = start().await else {
        return;
    };
    assert_eq!(fs.label(), "sftp://test@local");
    let d = tempfile::tempdir().expect("tmp");
    let root = d.path().canonicalize().expect("canon");
    let p = |s: &str| format!("{}/{s}", root.display());

    fs.mkdir(&p("dir")).await.expect("mkdir");
    let big: Vec<u8> = (0..300_000u32).map(|i| (i % 251) as u8).collect();
    put(&*fs, &p("dir/a.bin"), 0, &big).await;
    assert_eq!(std::fs::read(root.join("dir/a.bin")).expect("leer"), big);

    // Listar y stat.
    let l = fs.list(&p("dir")).await.expect("list");
    assert_eq!(l.len(), 1);
    assert_eq!(
        (l[0].name.as_str(), l[0].kind, l[0].size),
        ("a.bin", Kind::File, 300_000)
    );
    assert_eq!(l[0].path, p("dir/a.bin"));
    assert!(l[0].mode.is_some() && l[0].mtime.is_some());
    let root_l = fs
        .list(&root.display().to_string())
        .await
        .expect("list raíz");
    assert_eq!(root_l[0].kind, Kind::Dir);
    assert!(fs.stat(&p("nada")).await.expect("stat").is_none());

    // Lectura a mitad.
    assert_eq!(get(&*fs, &p("dir/a.bin"), 100_000).await, big[100_000..]);

    // Reanudación: subida cortada a 120_000 bytes y continuada.
    put(&*fs, &p("dir/r.bin"), 0, &big[..120_000]).await;
    put(&*fs, &p("dir/r.bin"), 120_000, &big[120_000..]).await;
    assert_eq!(std::fs::read(root.join("dir/r.bin")).expect("leer"), big);
    // Reanudar no trunca lo anterior.
    put(&*fs, &p("dir/r.bin"), 299_990, &big[299_990..]).await;
    assert_eq!(std::fs::read(root.join("dir/r.bin")).expect("leer"), big);

    // mtime, renombrar, borrar.
    fs.set_mtime(&p("dir/a.bin"), 1_700_000_000)
        .await
        .expect("mtime");
    let st = fs.stat(&p("dir/a.bin")).await.expect("stat");
    assert_eq!(st.expect("existe").mtime, Some(1_700_000_000));
    fs.rename(&p("dir/a.bin"), &p("dir/b.bin"))
        .await
        .expect("rename");
    assert!(fs.stat(&p("dir/a.bin")).await.expect("stat").is_none());
    fs.remove_file(&p("dir/b.bin")).await.expect("rm");
    fs.remove_file(&p("dir/r.bin")).await.expect("rm");
    fs.remove_dir(&p("dir")).await.expect("rmdir");
    assert!(!root.join("dir").exists());

    // Errores mapeados.
    let r = fs.open_read(&p("no-existe"), 0).await;
    assert!(matches!(r, Err(Error::NotFound(_))));
    let r = fs.remove_file(&p("no-existe")).await;
    assert!(matches!(r, Err(Error::NotFound(_))));
}

#[tokio::test]
async fn sftp_permiso_denegado() {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let Some((fs, _child)) = start().await else {
        return;
    };
    let d = tempfile::tempdir().expect("tmp");
    // Como root el permiso no se deniega.
    if std::fs::metadata(d.path()).expect("meta").uid() == 0 {
        return;
    }
    let ro = d.path().canonicalize().expect("canon").join("ro");
    std::fs::create_dir(&ro).expect("mkdir");
    std::fs::set_permissions(&ro, std::fs::Permissions::from_mode(0o500)).expect("chmod");
    let r = fs.mkdir(&format!("{}/x", ro.display())).await;
    std::fs::set_permissions(&ro, std::fs::Permissions::from_mode(0o700)).expect("chmod");
    assert!(matches!(r, Err(Error::PermissionDenied(_))), "{r:?}");
}
