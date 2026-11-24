use std::path::Path;

use friki_xfer::cache::DirCache;
use friki_xfer::local::LocalFs;
use friki_xfer::sync::{Action, Filters, SyncMode, SyncOptions, cache_key, plan};
use friki_xfer::{Kind, RemoteFs};

fn put(root: &Path, rel: &str, body: &str, mtime: i64) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(&p, body).unwrap();
    let f = std::fs::File::options().write(true).open(&p).unwrap();
    f.set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(mtime as u64))
        .unwrap();
}

fn opts(mode: SyncMode, delete: bool) -> SyncOptions {
    SyncOptions {
        mode,
        delete,
        filters: Filters::default(),
        mtime_tolerance: 2,
    }
}

async fn run(l: &Path, r: &Path, o: &SyncOptions, cache: Option<&DirCache>) -> Vec<Action> {
    plan(&LocalFs::new(l), "", &LocalFs::new(r), "", o, cache)
        .await
        .unwrap()
}

fn s(p: &str) -> String {
    p.to_string()
}

#[tokio::test]
async fn upload_con_filtros_y_borrados() {
    let (a, b) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    put(a.path(), "nuevo.txt", "n", 100);
    put(a.path(), "igual.txt", "i", 100);
    put(a.path(), "igual_cerca.txt", "i", 100);
    put(a.path(), "cambia.txt", "largo", 100);
    put(a.path(), "dir/dentro.txt", "d", 100);
    put(a.path(), "debug.log", "l", 100);
    put(a.path(), "node_modules/x/y.js", "y", 100);
    put(a.path(), "src/tmp/z", "z", 100);
    put(b.path(), "igual.txt", "i", 100);
    put(b.path(), "igual_cerca.txt", "i", 101); // dentro de la tolerancia
    put(b.path(), "cambia.txt", "c", 100);
    put(b.path(), "sobra.txt", "s", 100);
    put(b.path(), "vieja/a/b.txt", "s", 100);
    put(b.path(), "viejo.log", "s", 100); // excluido: no se borra

    let mut o = opts(SyncMode::Upload, true);
    o.filters.exclude = vec![s("*.log"), s("node_modules/"), s("**/tmp/*")];
    let acts = run(a.path(), b.path(), &o, None).await;
    assert_eq!(
        acts,
        vec![
            Action::Upload {
                path: s("cambia.txt")
            },
            Action::MkdirRemote { path: s("dir") },
            Action::Upload {
                path: s("dir/dentro.txt")
            },
            Action::Upload {
                path: s("nuevo.txt")
            },
            Action::MkdirRemote { path: s("src") },
            // `**/tmp/*` excluye el contenido de tmp, no el directorio.
            Action::MkdirRemote { path: s("src/tmp") },
            Action::DeleteRemote {
                path: s("sobra.txt")
            },
            Action::DeleteRemote {
                path: s("vieja/a/b.txt")
            },
            Action::DeleteRemote { path: s("vieja/a") },
            Action::DeleteRemote { path: s("vieja") },
        ]
    );

    // Sin `delete` no hay borrados; `include` restringe los ficheros; exclude gana.
    let mut o = opts(SyncMode::Upload, false);
    o.filters.include = vec![s("*.txt")];
    o.filters.exclude = vec![s("nuevo.txt")];
    let acts = run(a.path(), b.path(), &o, None).await;
    assert!(
        !acts
            .iter()
            .any(|a| matches!(a, Action::DeleteRemote { .. }))
    );
    assert!(!acts.contains(&Action::Upload {
        path: s("nuevo.txt")
    }));
    assert!(acts.contains(&Action::Upload {
        path: s("cambia.txt")
    }));
}

#[tokio::test]
async fn download_es_simetrico() {
    let (a, b) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    put(b.path(), "d/f.txt", "r", 100);
    put(a.path(), "viejo", "x", 100);
    let acts = run(a.path(), b.path(), &opts(SyncMode::Download, true), None).await;
    assert_eq!(
        acts,
        vec![
            Action::MkdirLocal { path: s("d") },
            Action::Download { path: s("d/f.txt") },
            Action::DeleteLocal { path: s("viejo") },
        ]
    );
}

#[tokio::test]
async fn both_gana_la_fecha_mas_reciente_y_no_borra() {
    let (a, b) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    put(a.path(), "solo_local", "1", 100);
    put(b.path(), "solo_remoto", "2", 100);
    put(a.path(), "local_nuevo", "aaa", 500);
    put(b.path(), "local_nuevo", "b", 100);
    put(a.path(), "remoto_nuevo", "a", 100);
    put(b.path(), "remoto_nuevo", "bbb", 500);
    let acts = run(a.path(), b.path(), &opts(SyncMode::Both, true), None).await;
    assert_eq!(
        acts,
        vec![
            Action::Upload {
                path: s("local_nuevo")
            },
            Action::Download {
                path: s("remoto_nuevo")
            },
            Action::Upload {
                path: s("solo_local")
            },
            Action::Download {
                path: s("solo_remoto")
            },
        ]
    );
}

#[tokio::test]
async fn both_con_cache_detecta_conflicto() {
    let (a, b) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let dir = tempfile::tempdir().unwrap();
    let (lfs, rfs) = (LocalFs::new(a.path()), LocalFs::new(b.path()));
    // Estado de la última sincronización: "f" medía 1 byte con mtime 100.
    put(a.path(), "f", "1", 100);
    put(a.path(), "g", "1", 100);
    let mut last = lfs.list("").await.unwrap();
    last.retain(|e| e.kind == Kind::File);
    let cache = DirCache::open(&dir.path().join("c.db")).unwrap();
    cache.store(&cache_key(&lfs, "", &rfs, ""), &last).unwrap();
    // `f` cambió en ambos lados; `g` solo en el remoto.
    put(a.path(), "f", "local", 300);
    put(b.path(), "f", "remoto!", 400);
    put(a.path(), "g", "1", 100);
    put(b.path(), "g", "nuevo", 200);
    let acts = run(
        a.path(),
        b.path(),
        &opts(SyncMode::Both, false),
        Some(&cache),
    )
    .await;
    assert_eq!(
        acts,
        vec![
            Action::Conflict { path: s("f") },
            Action::Download { path: s("g") },
        ]
    );
}

#[tokio::test]
async fn ignora_nombres_remotos_inseguros() {
    use friki_xfer::{Entry, Reader, Result, Writer};
    struct Evil;
    #[async_trait::async_trait]
    impl RemoteFs for Evil {
        async fn list(&self, _: &str) -> Result<Vec<Entry>> {
            let e = |n: &str| Entry {
                name: n.into(),
                path: n.into(),
                kind: Kind::File,
                size: 1,
                mtime: Some(1),
                mode: None,
            };
            Ok(vec![e("../x"), e("a/b"), e("bueno")])
        }
        async fn stat(&self, _: &str) -> Result<Option<Entry>> {
            Ok(None)
        }
        async fn mkdir(&self, _: &str) -> Result<()> {
            Ok(())
        }
        async fn remove_file(&self, _: &str) -> Result<()> {
            Ok(())
        }
        async fn remove_dir(&self, _: &str) -> Result<()> {
            Ok(())
        }
        async fn rename(&self, _: &str, _: &str) -> Result<()> {
            Ok(())
        }
        async fn open_read(&self, _: &str, _: u64) -> Result<Reader> {
            unimplemented!()
        }
        async fn open_write(&self, _: &str, _: u64) -> Result<Writer> {
            unimplemented!()
        }
        async fn set_mtime(&self, _: &str, _: i64) -> Result<()> {
            Ok(())
        }
        fn label(&self) -> String {
            "evil".into()
        }
    }
    let a = tempfile::tempdir().unwrap();
    let acts = plan(
        &LocalFs::new(a.path()),
        "",
        &Evil,
        "",
        &opts(SyncMode::Download, false),
        None,
    )
    .await
    .unwrap();
    assert_eq!(acts, vec![Action::Download { path: s("bueno") }]);
}
