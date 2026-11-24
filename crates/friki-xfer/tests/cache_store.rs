use friki_xfer::cache::DirCache;
use friki_xfer::{Entry, Kind};

fn e(path: &str, kind: Kind, size: u64) -> Entry {
    Entry {
        name: path.rsplit('/').next().unwrap().into(),
        path: path.into(),
        kind,
        size,
        mtime: Some(42),
        mode: Some(0o644),
    }
}

#[test]
fn guarda_y_carga_por_clave_con_permisos_0600() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("c.db");
    let c = DirCache::open(&p).unwrap();
    let a = vec![e("d", Kind::Dir, 0), e("d/f", Kind::File, 7)];
    c.store("k1", &a).unwrap();
    c.store("k2", &[e("z", Kind::File, 1)]).unwrap();
    assert_eq!(c.load("k1").unwrap(), a);
    assert!(c.load("nada").unwrap().is_empty());
    // `store` sustituye el estado anterior y persiste al reabrir.
    c.store("k1", &a[..1]).unwrap();
    drop(c);
    let c = DirCache::open(&p).unwrap();
    assert_eq!(c.load("k1").unwrap(), a[..1]);
    assert_eq!(c.load("k2").unwrap().len(), 1);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&p).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}
