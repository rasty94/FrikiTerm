#![cfg(unix)]

use friki_xfer::local::LocalFs;
use friki_xfer::{Error, Kind, RemoteFs};

#[tokio::test]
async fn rechaza_rutas_que_salen_de_la_raiz() {
    let (a, out) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    std::fs::write(out.path().join("secreto"), b"x").unwrap();
    std::fs::write(a.path().join("ok"), b"hola").unwrap();
    std::os::unix::fs::symlink(out.path(), a.path().join("escape")).unwrap();
    std::os::unix::fs::symlink(out.path().join("secreto"), a.path().join("enlace")).unwrap();
    let fs = LocalFs::new(a.path());

    for p in ["../x", "a/../../x", "/etc/passwd", "escape/secreto"] {
        assert!(
            matches!(fs.stat(p).await, Err(Error::PermissionDenied(_))),
            "{p}"
        );
    }
    assert!(matches!(
        fs.open_read("enlace", 0).await,
        Err(Error::PermissionDenied(_))
    ));
    assert!(matches!(
        fs.list("escape").await,
        Err(Error::PermissionDenied(_))
    ));
    assert!(matches!(
        fs.open_write("escape/nuevo", 0).await,
        Err(Error::PermissionDenied(_))
    ));
    assert!(!out.path().join("nuevo").exists());

    // El propio enlace se ve como enlace y no se sigue.
    assert_eq!(
        fs.stat("enlace").await.unwrap().unwrap().kind,
        Kind::Symlink
    );
    let names: Vec<_> = fs
        .list("")
        .await
        .unwrap()
        .into_iter()
        .map(|e| e.name)
        .collect();
    assert_eq!(names, ["enlace", "escape", "ok"]);
    assert_eq!(fs.stat("nada").await.unwrap(), None);
    assert_eq!(fs.stat("ok").await.unwrap().unwrap().size, 4);
}
