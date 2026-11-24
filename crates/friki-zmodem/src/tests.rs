#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::atomic::{AtomicU32, Ordering};

use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream, duplex};

use super::*;
use crate::z::{Hdr, ZDATA, ZFILE, bin_header, data_subpacket, hex_header};

static N: AtomicU32 = AtomicU32::new(0);

fn tmpdir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!(
        "friki-zm-{}-{}-{tag}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn pattern(len: usize, seed: u32) -> Vec<u8> {
    let mut x = seed.wrapping_mul(2_654_435_761).max(1);
    let mut v: Vec<u8> = (0..len)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            (x >> 8) as u8
        })
        .collect();
    if let Some(l) = v.last_mut() {
        *l = 0x41; // Xmodem recorta los 0x1A finales
    }
    v
}

/// Relé que invierte bits del byte `flip_at` del sentido emisor -> receptor.
fn relay(s: DuplexStream, r: DuplexStream, flip_at: usize) {
    let (mut sr, mut sw) = tokio::io::split(s);
    let (mut rr, mut rw) = tokio::io::split(r);
    tokio::spawn(async move {
        let mut buf = [0u8; 512];
        let mut count = 0usize;
        loop {
            let n = sr.read(&mut buf).await.unwrap_or(0);
            if n == 0 {
                break;
            }
            for (i, b) in buf[..n].iter_mut().enumerate() {
                if count + i == flip_at {
                    *b ^= 0x55;
                }
            }
            count += n;
            if rw.write_all(&buf[..n]).await.is_err() {
                break;
            }
        }
    });
    tokio::spawn(async move {
        let _ = tokio::io::copy(&mut rr, &mut sw).await;
    });
}

async fn transfer(proto: FileProtocol, files: &[(&str, usize)], corrupt: Option<usize>) {
    let src = tmpdir("src");
    let dst = tmpdir("dst");
    let mut paths = Vec::new();
    let mut contents = Vec::new();
    for (i, (name, len)) in files.iter().enumerate() {
        let data = pattern(*len, i as u32 + 7);
        let p = src.join(name);
        std::fs::write(&p, &data).unwrap();
        paths.push(p);
        contents.push(data);
    }
    let (mut a, mut b) = duplex(1 << 16);
    if let Some(at) = corrupt {
        let (a2, ra) = duplex(1 << 16);
        let (b2, rb) = duplex(1 << 16);
        relay(ra, rb, at);
        a = a2;
        b = b2;
    }
    let mut pr = |_: Progress| {};
    let mut pr2 = |_: Progress| {};
    let (s, r) = tokio::join!(
        send(&mut a, proto, &paths, &mut pr),
        receive(&mut b, proto, &dst, &mut pr2)
    );
    s.unwrap();
    let got = r.unwrap();
    assert_eq!(got.len(), files.len());
    for (g, want) in got.iter().zip(&contents) {
        assert!(g.starts_with(&dst));
        assert_eq!(&std::fs::read(g).unwrap(), want, "{g:?}");
    }
}

const BIG: usize = 2 * 1024 * 1024 + 5;

#[tokio::test]
async fn xmodem_roundtrip() {
    for len in [0, 1, 127, 128, 129, BIG] {
        transfer(FileProtocol::Xmodem, &[("f.bin", len)], None).await;
    }
}

#[tokio::test]
async fn ymodem_batch() {
    transfer(
        FileProtocol::Ymodem,
        &[("a", 0), ("b", 1), ("c", 300), ("d", BIG)],
        None,
    )
    .await;
}

#[tokio::test]
async fn zmodem_batch() {
    transfer(
        FileProtocol::Zmodem,
        &[("a", 0), ("b", 1), ("c", 1024), ("d", BIG)],
        None,
    )
    .await;
}

#[tokio::test]
async fn corrupted_block_is_retried() {
    for p in [
        FileProtocol::Xmodem,
        FileProtocol::Ymodem,
        FileProtocol::Zmodem,
    ] {
        transfer(p, &[("f", 5000)], Some(600)).await;
    }
    // También en mitad de un fichero grande con varias ventanas Zmodem.
    transfer(FileProtocol::Zmodem, &[("f", 100_000)], Some(40_000)).await;
}

#[test]
fn sanitize_cases() {
    assert_eq!(sanitize_name(b"../../etc/passwd"), "passwd");
    assert_eq!(sanitize_name(b"..\\..\\win\\x.txt"), "x.txt");
    assert_eq!(sanitize_name(b"/abs/olute"), "olute");
    assert_eq!(sanitize_name(b".."), "file");
    assert_eq!(sanitize_name(b""), "file");
    assert_eq!(sanitize_name(b"a/"), "file");
    assert_eq!(sanitize_name(b"a\x00b\x1b[31m\n"), "ab[31m");
    assert_eq!(sanitize_name(b".hidden"), "hidden");
}

#[test]
fn crc_vectors() {
    assert_eq!(crc16_update(0, b"123456789"), 0x31c3);
    assert_eq!(!crc32_update(!0, b"123456789"), 0xcbf4_3926);
}

#[test]
fn detect_cases() {
    assert_eq!(
        detect(b"hola\r\n**\x18B00000000000000\r\x8a"),
        Some(ZmodemStart::RemoteSends)
    );
    assert_eq!(
        detect(b"**\x18B0100000023be50\r\x8a\x11"),
        Some(ZmodemStart::RemoteReceives)
    );
    assert_eq!(detect(b"nada que ver"), None);
}

fn xblock(seq: u8, payload: &[u8], size: usize, pad: u8) -> Vec<u8> {
    let mut d = payload.to_vec();
    d.resize(size, pad);
    let mut p = vec![if size == 128 { 1 } else { 2 }, seq, !seq];
    p.extend(&d);
    p.extend(crc16_update(0, &d).to_be_bytes());
    p
}

#[tokio::test]
async fn ymodem_malicious_names_never_escape_or_overwrite() {
    let root = tmpdir("evil");
    let dst = root.join("dst");
    for round in 0..2 {
        let (mut a, mut b) = duplex(1 << 16);
        let mut pr = |_: Progress| {};
        let sender = async {
            let mut one = [0u8; 1];
            a.read_exact(&mut one).await.unwrap();
            // Tamaño anunciado enorme: no debe reservarse memoria.
            a.write_all(&xblock(0, b"../../evil\0 99999999999999", 128, 0))
                .await
                .unwrap();
            let mut two = [0u8; 2];
            a.read_exact(&mut two).await.unwrap(); // ACK + 'C'
            a.write_all(&xblock(1, b"abc", 128, 0x1a)).await.unwrap();
            a.read_exact(&mut one).await.unwrap(); // ACK
            a.write_all(&[4]).await.unwrap();
            a.read_exact(&mut one).await.unwrap(); // NAK
            a.write_all(&[4]).await.unwrap();
            a.read_exact(&mut two).await.unwrap(); // ACK + 'C'
            a.write_all(&xblock(0, b"", 128, 0)).await.unwrap();
            a.read_exact(&mut one).await.unwrap();
        };
        let (_, r) = tokio::join!(sender, receive(&mut b, FileProtocol::Ymodem, &dst, &mut pr));
        let got = r.unwrap();
        assert_eq!(got.len(), 1);
        let want = if round == 0 { "evil" } else { "evil.1" };
        assert_eq!(got[0], dst.join(want));
        assert!(!root.join("evil").exists());
    }
}

fn zblob(files: &[(&[u8], &[u8])]) -> Vec<u8> {
    let mut v = Vec::new();
    for (name, body) in files {
        let mut info = name.to_vec();
        info.extend(b"\0 99999999999999 0 0");
        v.extend(bin_header(&Hdr::flags(ZFILE, 1), true));
        v.extend(data_subpacket(&info, b'k', true));
        v.extend(bin_header(&Hdr::pos(ZDATA, 0), true));
        v.extend(data_subpacket(body, b'h', true));
        v.extend(bin_header(&Hdr::pos(11, body.len() as u64), true));
    }
    v
}

#[tokio::test]
async fn zmodem_malicious_names_and_batch_limit() {
    let root = tmpdir("zevil");
    let dst = root.join("dst");
    let (mut a, mut b) = duplex(1 << 20);
    let mut blob = zblob(&[
        (b"../../evil", b"abc"),
        (b"..\\..\\evil", b"def"),
        (b"/etc/pass\x07wd\x1b", b"ghi"),
        (b"evil", b"jkl"),
    ]);
    blob.extend(hex_header(&Hdr::pos(8, 0)));
    a.write_all(&blob).await.unwrap();
    let mut pr = |_: Progress| {};
    let got = receive(&mut b, FileProtocol::Zmodem, &dst, &mut pr)
        .await
        .unwrap();
    let names: Vec<_> = got
        .iter()
        .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, ["evil", "evil.1", "passwd", "evil.2"]);
    assert_eq!(std::fs::read(&got[0]).unwrap(), b"abc");
    assert_eq!(std::fs::read(&got[3]).unwrap(), b"jkl");
    assert!(!root.join("evil").exists());

    // Límite de ficheros por lote.
    let dst2 = root.join("dst2");
    let many: Vec<(Vec<u8>, &[u8])> = (0..=MAX_FILES)
        .map(|i| (format!("f{i}").into_bytes(), &b""[..]))
        .collect();
    let refs: Vec<(&[u8], &[u8])> = many.iter().map(|(n, b)| (n.as_slice(), *b)).collect();
    let (mut a, mut b) = duplex(1 << 22);
    a.write_all(&zblob(&refs)).await.unwrap();
    let e = receive(&mut b, FileProtocol::Zmodem, &dst2, &mut pr)
        .await
        .unwrap_err();
    assert_eq!(e.kind(), io::ErrorKind::InvalidData);
}

// --- Interoperabilidad con lrzsz (`sz`/`rz`), si están instalados -----------------------

fn have(cmd: &str) -> bool {
    std::process::Command::new(cmd)
        .arg("--version")
        .output()
        .is_ok()
}

fn spawn(cmd: &str, args: &[&str], cwd: &Path) -> tokio::process::Child {
    tokio::process::Command::new(cmd)
        .args(args)
        .current_dir(cwd)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .unwrap()
}

async fn sz_to_us(flag: &str, proto: FileProtocol) {
    if !have("sz") {
        return;
    }
    let src = tmpdir("isrc");
    let dst = tmpdir("idst");
    let data = pattern(300_000, 3);
    std::fs::write(src.join("f.bin"), &data).unwrap();
    let mut child = spawn("sz", &[flag, "f.bin"], &src);
    let mut io = tokio::io::join(child.stdout.take().unwrap(), child.stdin.take().unwrap());
    let mut pr = |_: Progress| {};
    let got = receive(&mut io, proto, &dst, &mut pr).await.unwrap();
    assert_eq!(std::fs::read(&got[0]).unwrap(), data);
}

async fn us_to_rz(flag: &str, proto: FileProtocol) {
    if !have("rz") {
        return;
    }
    let src = tmpdir("isrc");
    let dst = tmpdir("idst");
    let data = pattern(300_000, 4);
    std::fs::write(src.join("f.bin"), &data).unwrap();
    let mut child = spawn("rz", &[flag], &dst);
    let mut io = tokio::io::join(child.stdout.take().unwrap(), child.stdin.take().unwrap());
    let mut pr = |_: Progress| {};
    send(&mut io, proto, &[src.join("f.bin")], &mut pr)
        .await
        .unwrap();
    assert_eq!(std::fs::read(dst.join("f.bin")).unwrap(), data);
}

#[tokio::test]
#[ignore = "requiere lrzsz (sz/rz)"]
async fn interop_sz_zmodem() {
    sz_to_us("--zmodem", FileProtocol::Zmodem).await;
}

#[tokio::test]
#[ignore = "requiere lrzsz (sz/rz)"]
async fn interop_sz_ymodem() {
    sz_to_us("--ymodem", FileProtocol::Ymodem).await;
}

#[tokio::test]
#[ignore = "requiere lrzsz (sz/rz)"]
async fn interop_rz_zmodem() {
    us_to_rz("--zmodem", FileProtocol::Zmodem).await;
}

#[tokio::test]
#[ignore = "requiere lrzsz (sz/rz)"]
async fn interop_rz_ymodem() {
    us_to_rz("--ymodem", FileProtocol::Ymodem).await;
}
