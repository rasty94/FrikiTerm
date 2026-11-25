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
        max_deletes: None,
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

#[tokio::test]
async fn origen_inexistente_es_error_y_no_borra() {
    let a = tempfile::tempdir().unwrap();
    put(a.path(), "x.txt", "x", 100);
    let gone = a.path().join("no-existe");
    let (l, r) = (LocalFs::new(a.path()), LocalFs::new(a.path()));
    // Download con delete: el origen (remoto) no existe -> error, no DeleteLocal de todo.
    let o = opts(SyncMode::Download, true);
    assert!(
        plan(
            &l,
            "",
            &r,
            gone.file_name().unwrap().to_str().unwrap(),
            &o,
            None
        )
        .await
        .is_err()
    );
    // Upload: el origen es el local.
    let o = opts(SyncMode::Upload, true);
    assert!(plan(&l, "no-existe", &r, "", &o, None).await.is_err());
    // Un destino inexistente sigue siendo un árbol vacío.
    let o = opts(SyncMode::Upload, true);
    let acts = plan(&l, "", &r, "no-existe", &o, None).await.unwrap();
    assert!(acts.iter().any(|a| matches!(a, Action::Upload { .. })));
}

#[tokio::test]
async fn max_deletes_limita_el_plan() {
    let (a, b) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    put(b.path(), "a", "1", 100);
    put(b.path(), "b", "1", 100);
    let mut o = opts(SyncMode::Upload, true);
    o.max_deletes = Some(1);
    let (l, r) = (LocalFs::new(a.path()), LocalFs::new(b.path()));
    let e = plan(&l, "", &r, "", &o, None).await.unwrap_err();
    assert!(e.to_string().contains("máximo"), "{e}");
    o.max_deletes = Some(2);
    assert_eq!(plan(&l, "", &r, "", &o, None).await.unwrap().len(), 2);
}

#[tokio::test]
async fn both_con_cache_propaga_borrados() {
    let (a, b, d) = (
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
    );
    for root in [a.path(), b.path()] {
        put(root, "borrado_r.txt", "x", 100);
        put(root, "borrado_l.txt", "x", 100);
        put(root, "cambiado.txt", "x", 100);
        put(root, "dir/x.txt", "x", 100);
    }
    let (lfs, rfs) = (LocalFs::new(a.path()), LocalFs::new(b.path()));
    let cache = DirCache::open(&d.path().join("c.db")).unwrap();
    let last = friki_xfer::sync::snapshot(&lfs, "").await.unwrap();
    cache.store(&cache_key(&lfs, "", &rfs, ""), &last).unwrap();
    // El remoto borra borrado_r y dir; el local borra borrado_l; `cambiado` se modifica en
    // local y se borra en remoto (se conserva); nuevo.txt solo existe en local.
    std::fs::remove_file(b.path().join("borrado_r.txt")).unwrap();
    std::fs::remove_dir_all(b.path().join("dir")).unwrap();
    std::fs::remove_file(a.path().join("borrado_l.txt")).unwrap();
    std::fs::remove_file(b.path().join("cambiado.txt")).unwrap();
    put(a.path(), "cambiado.txt", "mucho más largo", 200);
    put(a.path(), "nuevo.txt", "n", 100);

    let o = opts(SyncMode::Both, true);
    let acts = run(a.path(), b.path(), &o, Some(&cache)).await;
    assert_eq!(
        acts,
        vec![
            Action::Upload {
                path: s("cambiado.txt")
            },
            Action::Upload {
                path: s("nuevo.txt")
            },
            Action::DeleteRemote {
                path: s("borrado_l.txt")
            },
            Action::DeleteLocal {
                path: s("borrado_r.txt")
            },
            Action::DeleteLocal {
                path: s("dir/x.txt")
            },
            Action::DeleteLocal { path: s("dir") },
        ]
    );
    // Sin `delete` nada se borra: se restaura.
    let o = opts(SyncMode::Both, false);
    let acts = run(a.path(), b.path(), &o, Some(&cache)).await;
    assert!(
        acts.iter()
            .all(|a| !matches!(a, Action::DeleteLocal { .. } | Action::DeleteRemote { .. }))
    );
}

#[tokio::test]
async fn both_con_cache_aborta_si_una_raiz_falta_o_se_vacia() {
    let (a, b, d) = (
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
    );
    put(a.path(), "x.txt", "x", 100);
    put(b.path(), "x.txt", "x", 100);
    let (lfs, rfs) = (LocalFs::new(a.path()), LocalFs::new(b.path()));
    let cache = DirCache::open(&d.path().join("c.db")).unwrap();
    let last = friki_xfer::sync::snapshot(&lfs, "").await.unwrap();
    cache.store(&cache_key(&lfs, "", &rfs, ""), &last).unwrap();
    let o = opts(SyncMode::Both, true);
    assert!(plan(&lfs, "", &rfs, "", &o, Some(&cache)).await.is_ok());

    // Raíz local desaparecida (disco desmontado): no debe planificar borrados remotos.
    std::fs::remove_dir_all(a.path()).unwrap();
    let e = plan(&lfs, "", &rfs, "", &o, Some(&cache))
        .await
        .unwrap_err();
    assert!(matches!(e, friki_xfer::Error::UnsafeSync(_)), "{e}");

    // Raíz remota vaciada: lo mismo, por el otro lado.
    put(a.path(), "x.txt", "x", 100);
    std::fs::remove_file(b.path().join("x.txt")).unwrap();
    let e = plan(&lfs, "", &rfs, "", &o, Some(&cache))
        .await
        .unwrap_err();
    assert!(matches!(e, friki_xfer::Error::UnsafeSync(_)), "{e}");
}
