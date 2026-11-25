use super::*;

/// Resultado de una transferencia de cliente: datos o `(código, mensaje)` de ERROR.
type Out = Result<Vec<u8>, (u16, String)>;

async fn start(
    dir: &Path,
    allow_write: bool,
    overwrite: bool,
) -> (SocketAddr, tokio::task::JoinHandle<()>) {
    start_max(dir, allow_write, overwrite, 1 << 30).await
}

async fn start_max(
    dir: &Path,
    allow_write: bool,
    overwrite: bool,
    max_size: u64,
) -> (SocketAddr, tokio::task::JoinHandle<()>) {
    let sock = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let addr = sock.local_addr().unwrap();
    let cfg = Config {
        root: dir.to_path_buf(),
        allow_write,
        overwrite,
        max_size,
        log: |_| {},
    };
    (
        addr,
        tokio::spawn(async move { drop(serve(sock, cfg).await) }),
    )
}

fn rq(op: u8, name: &str, opts: &[(&str, &str)]) -> Vec<u8> {
    let mut p = vec![0, op];
    p.extend_from_slice(format!("{name}\0octet\0").as_bytes());
    for (k, v) in opts {
        p.extend_from_slice(format!("{k}\0{v}\0").as_bytes());
    }
    p
}

fn err_of(p: &[u8]) -> (u16, String) {
    (
        u16::from_be_bytes([p[2], p[3]]),
        String::from_utf8_lossy(&p[4..p.len() - 1]).into(),
    )
}

/// Cliente mínimo de lectura; devuelve también el OACK si lo hubo.
async fn get(srv: SocketAddr, name: &str, opts: &[(&str, &str)]) -> (Out, Option<Vec<u8>>) {
    let s = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    s.send_to(&rq(1, name, opts), srv).await.unwrap();
    let (mut out, mut oack, mut buf) = (Vec::new(), None, vec![0u8; 70000]);
    let mut blk = 512;
    loop {
        let (n, from) = timeout(Duration::from_secs(5), s.recv_from(&mut buf))
            .await
            .unwrap()
            .unwrap();
        let p = &buf[..n];
        match p[1] {
            5 => return (Err(err_of(p)), oack),
            6 => {
                oack = Some(p.to_vec());
                if let Some(v) = opts.iter().find(|o| o.0 == "blksize") {
                    blk = v.1.parse().unwrap();
                }
                s.send_to(&[0, 4, 0, 0], from).await.unwrap();
            }
            _ => {
                out.extend_from_slice(&p[4..]);
                s.send_to(&[0, 4, p[2], p[3]], from).await.unwrap();
                if n - 4 < blk {
                    return (Ok(out), oack);
                }
            }
        }
    }
}

/// Cliente mínimo de escritura con bloques de 512.
async fn put(srv: SocketAddr, name: &str, data: &[u8]) -> Result<(), (u16, String)> {
    let s = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    s.send_to(&rq(2, name, &[]), srv).await.unwrap();
    let mut buf = vec![0u8; 600];
    let mut chunks: Vec<&[u8]> = data.chunks(512).collect();
    if data.len().is_multiple_of(512) {
        chunks.push(&[]);
    }
    for (i, c) in chunks.iter().enumerate() {
        let (n, from) = timeout(Duration::from_secs(5), s.recv_from(&mut buf))
            .await
            .unwrap()
            .unwrap();
        if buf[1] == 5 {
            return Err(err_of(&buf[..n]));
        }
        assert!(is_pkt(&buf[..n], 4, i as u16));
        s.send_to(&block_pkt(3, i as u16 + 1, c), from)
            .await
            .unwrap();
    }
    let n = timeout(Duration::from_secs(5), s.recv(&mut buf))
        .await
        .unwrap()
        .unwrap();
    assert!(is_pkt(&buf[..n], 4, chunks.len() as u16));
    Ok(())
}

#[tokio::test]
async fn rrq_y_blksize() {
    let d = tempfile::tempdir().unwrap();
    let data: Vec<u8> = (0..5000u32).map(|i| i as u8).collect();
    std::fs::write(d.path().join("fw.bin"), &data).unwrap();
    let (addr, _srv) = start(d.path(), false, false).await;
    assert_eq!(get(addr, "fw.bin", &[]).await.0.unwrap(), data);
    let (out, oack) = get(addr, "fw.bin", &[("blksize", "1024"), ("tsize", "0")]).await;
    assert_eq!(out.unwrap(), data);
    assert_eq!(oack.unwrap(), b"\0\x06blksize\x001024\0tsize\x005000\0");
    assert_eq!(get(addr, "no.bin", &[]).await.0.unwrap_err().0, 1);
}

#[tokio::test]
async fn wrq_permitido_y_sin_sobrescribir() {
    let d = tempfile::tempdir().unwrap();
    let (addr, _srv) = start(d.path(), true, false).await;
    let data = vec![7u8; 1024]; // múltiplo del bloque: exige bloque final vacío
    put(addr, "up.bin", &data).await.unwrap();
    assert_eq!(std::fs::read(d.path().join("up.bin")).unwrap(), data);
    assert_eq!(put(addr, "up.bin", b"x").await.unwrap_err().0, 6);
    assert_eq!(std::fs::read(d.path().join("up.bin")).unwrap(), data);
    // sin restos temporales
    assert_eq!(std::fs::read_dir(d.path()).unwrap().count(), 1);
}

#[tokio::test]
async fn wrq_con_overwrite() {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("a"), "viejo").unwrap();
    let (addr, _srv) = start(d.path(), true, true).await;
    put(addr, "a", b"nuevo").await.unwrap();
    assert_eq!(std::fs::read(d.path().join("a")).unwrap(), b"nuevo");
}

#[tokio::test]
async fn wrq_rechazado_por_defecto() {
    let d = tempfile::tempdir().unwrap();
    let (addr, _srv) = start(d.path(), false, false).await;
    assert_eq!(put(addr, "x", b"hola").await.unwrap_err().0, 2);
    assert!(!d.path().join("x").exists());
}

#[tokio::test]
async fn recorrido_de_rutas_rechazado() {
    let d = tempfile::tempdir().unwrap();
    let root = d.path().join("root");
    std::fs::create_dir(&root).unwrap();
    std::fs::write(d.path().join("secreto"), "s").unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(d.path().join("secreto"), root.join("enlace")).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(d.path(), root.join("dir")).unwrap();
    let (addr, _srv) = start(&root, true, true).await;
    for n in [
        "../secreto",
        "/etc/passwd",
        "a/../../secreto",
        "..\\secreto",
        "dir/secreto",
        "enlace",
    ] {
        if cfg!(not(unix)) && matches!(n, "dir/secreto" | "enlace") {
            continue;
        }
        assert_eq!(get(addr, n, &[]).await.0.unwrap_err().0, 2, "{n}");
    }
    assert_eq!(put(addr, "../fuera", b"x").await.unwrap_err().0, 2);
    #[cfg(unix)]
    assert_eq!(put(addr, "dir/fuera", b"x").await.unwrap_err().0, 2);
    assert!(!d.path().join("fuera").exists());
}

#[cfg(unix)]
#[tokio::test]
async fn subida_sustituye_el_enlace_no_su_destino() {
    let d = tempfile::tempdir().unwrap();
    let root = d.path().join("root");
    std::fs::create_dir(&root).unwrap();
    std::fs::write(d.path().join("secreto"), "s").unwrap();
    std::os::unix::fs::symlink(d.path().join("secreto"), root.join("enlace")).unwrap();
    let (addr, _srv) = start(&root, true, true).await;
    put(addr, "enlace", b"nuevo").await.unwrap();
    assert_eq!(std::fs::read(d.path().join("secreto")).unwrap(), b"s");
    assert!(
        !root
            .join("enlace")
            .symlink_metadata()
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(std::fs::read(root.join("enlace")).unwrap(), b"nuevo");
}

#[tokio::test]
async fn subida_con_tamano_maximo() {
    let d = tempfile::tempdir().unwrap();
    let (addr, _srv) = start_max(d.path(), true, false, 1000).await;
    assert_eq!(put(addr, "up.bin", &[3u8; 2000]).await.unwrap_err().0, 3);
    // el temporal se borra al abortar
    assert_eq!(std::fs::read_dir(d.path()).unwrap().count(), 0);
    put(addr, "ok.bin", &[3u8; 1000]).await.unwrap();
    assert_eq!(std::fs::read(d.path().join("ok.bin")).unwrap().len(), 1000);
}

#[tokio::test]
async fn plazo_negociado_acotado() {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("fw.bin"), b"x").unwrap();
    let (addr, _srv) = start(d.path(), false, false).await;
    let (out, oack) = get(addr, "fw.bin", &[("timeout", "255")]).await;
    assert_eq!(out.unwrap(), b"x");
    assert_eq!(oack.unwrap(), b"\0\x06timeout\x0010\0");
}

#[tokio::test]
async fn transferencias_simultaneas_limitadas() {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("big.bin"), vec![1u8; 4000]).unwrap();
    let (addr, _srv) = start(d.path(), false, false).await;
    // Clientes que piden y no contestan: cada transferencia queda ocupando un hueco.
    let mut hold = Vec::new();
    for _ in 0..MAX_TRANSFERS {
        let s = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        s.send_to(&rq(1, "big.bin", &[]), addr).await.unwrap();
        hold.push(s);
    }
    let (out, _) = get(addr, "big.bin", &[]).await;
    assert_eq!(out.unwrap_err().0, 0);
    drop(hold);
}
