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

#[tokio::test]
async fn zmodem_leftover_after_oo_and_total_cap() {
    let root = tmpdir("zex");
    let mut pr = |_: Progress| {};
    let (mut a, mut b) = duplex(1 << 20);
    let mut blob = zblob(&[(b"f", b"abc")]);
    blob.extend(hex_header(&Hdr::pos(8, 0)));
    blob.extend(b"OOuser@host$ ");
    a.write_all(&blob).await.unwrap();
    let r = receive_ex(&mut b, FileProtocol::Zmodem, &root.join("a"), &mut pr, 1000)
        .await
        .unwrap();
    assert_eq!(r.files.len(), 1);
    assert_eq!(r.leftover, b"user@host$ ");

    // Tope total superado: error y parcial borrado.
    let dst = root.join("b");
    let (mut a, mut b) = duplex(1 << 20);
    a.write_all(&zblob(&[(b"g", b"abcdef")])).await.unwrap();
    let e = receive_ex(&mut b, FileProtocol::Zmodem, &dst, &mut pr, 4)
        .await
        .unwrap_err();
    assert_eq!(e.kind(), io::ErrorKind::QuotaExceeded);
    assert_eq!(std::fs::read_dir(&dst).unwrap().count(), 0);
}

#[test]
fn sanitize_hostile_names() {
    assert_eq!(sanitize_name(b"CON"), "_CON");
    assert_eq!(sanitize_name(b"nul.txt"), "_nul.txt");
    assert_eq!(sanitize_name(b"com1"), "_com1");
    assert_eq!(sanitize_name(b"a:b"), "a_b");
    assert_eq!(sanitize_name(b"x. . "), "x");
    assert_eq!(sanitize_name("a\u{202E}txt.exe".as_bytes()), "atxt.exe");
    assert_eq!(sanitize_name(b"console"), "console");
}

// ---- Kermit ----

use crate::kermit::{self, Cfg};

fn kcfg() -> Cfg {
    Cfg {
        timeout: Duration::from_millis(300),
        retries: 20,
        ..Cfg::default()
    }
}

/// Datos con binario, controles, prefijos del protocolo y rachas largas.
fn kdata() -> Vec<u8> {
    let mut v = pattern(5000, 3);
    v.extend(vec![0u8; 500]);
    v.extend(vec![b'#'; 200]);
    v.extend(vec![b'~'; 50]);
    v.extend(vec![0xffu8; 300]);
    v.extend(vec![b'&'; 60]);
    v.extend(b"\r\n\x01\x03\x7f\x80\x81");
    v.extend(pattern(3000, 4));
    v
}

async fn ktransfer(cs: Cfg, cr: Cfg, flip: Option<usize>) {
    let (src, dst) = (tmpdir("ksrc"), tmpdir("kdst"));
    let data = kdata();
    let p = src.join("f.bin");
    std::fs::write(&p, &data).unwrap();
    let empty = src.join("empty");
    std::fs::write(&empty, b"").unwrap();
    let (mut a, mut b) = duplex(1 << 16);
    if let Some(at) = flip {
        let (a2, ra) = duplex(1 << 16);
        let (b2, rb) = duplex(1 << 16);
        relay(ra, rb, at);
        a = a2;
        b = b2;
    }
    let files = [p, empty];
    let (mut p1, mut p2) = (|_: Progress| {}, |_: Progress| {});
    let (s, r) = tokio::join!(
        kermit::send(&mut a, &files, &mut p1, cs),
        kermit::receive(&mut b, &dst, &mut p2, u64::MAX, cr)
    );
    s.unwrap();
    let (got, _) = r.unwrap();
    assert_eq!(got.len(), 2);
    assert_eq!(std::fs::read(&got[0]).unwrap(), data);
    assert!(std::fs::read(&got[1]).unwrap().is_empty());
}

#[tokio::test]
async fn kermit_loopback_default_and_variants() {
    let d = kcfg();
    ktransfer(d, d, None).await;
    // bloque tipo 1, sin repeticiones, 8.º bit con prefijo `&`
    let basic = Cfg {
        chk3: false,
        rept: false,
        qbin: b'&',
        ..d
    };
    ktransfer(basic, basic, None).await;
    // la propuesta de un lado basta si el otro acepta (`Y`)
    let q = Cfg { qbin: b'&', ..d };
    ktransfer(q, d, None).await;
    ktransfer(d, q, None).await;
    ktransfer(q, Cfg { qbin: b'N', ..d }, None).await;
    // un lado sin CRC ni repeticiones obliga al otro a bajar a tipo 1
    ktransfer(d, basic, None).await;
}

#[tokio::test]
async fn kermit_corruption_is_retried() {
    for at in [3, 40, 100, 700, 2500] {
        ktransfer(kcfg(), kcfg(), Some(at)).await;
    }
}

#[tokio::test]
async fn kermit_big_file_with_default_cfg() {
    transfer(
        FileProtocol::Kermit,
        &[("a", 0), ("b", 1), ("c", 300_000)],
        None,
    )
    .await;
}

fn kpkt(seq: u8, typ: u8, data: &[u8]) -> Vec<u8> {
    kermit::test_packet(seq, typ, data)
}

#[tokio::test]
async fn kermit_error_and_ctrl_c_abort() {
    let dst = tmpdir("kabort");
    for bytes in [kpkt(0, b'E', b"disco lleno"), vec![3, 3, 3]] {
        let (mut a, mut b) = duplex(4096);
        a.write_all(&bytes).await.unwrap();
        let mut p = |_: Progress| {};
        let e = kermit::receive(&mut b, &dst, &mut p, u64::MAX, kcfg())
            .await
            .unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::ConnectionAborted);
    }
    // E del receptor hacia el emisor
    let f = dst.join("x");
    std::fs::write(&f, b"x").unwrap();
    let (mut a, mut b) = duplex(4096);
    a.write_all(&kpkt(0, b'E', b"no")).await.unwrap();
    let mut p = |_: Progress| {};
    let e = kermit::send(&mut b, &[f], &mut p, kcfg())
        .await
        .unwrap_err();
    assert_eq!(e.kind(), io::ErrorKind::ConnectionAborted);
}

#[tokio::test]
async fn kermit_silent_peer_times_out() {
    let (_a, mut b) = duplex(4096);
    let cfg = Cfg {
        timeout: Duration::from_millis(20),
        retries: 2,
        ..Cfg::default()
    };
    let mut p = |_: Progress| {};
    let e = kermit::receive(&mut b, &tmpdir("ksil"), &mut p, 1, cfg)
        .await
        .unwrap_err();
    assert_eq!(e.kind(), io::ErrorKind::TimedOut);
}

#[tokio::test]
async fn kermit_f_sin_z_descarta_el_parcial() {
    let dst = tmpdir("kparcial");
    let (mut a, mut b) = duplex(1 << 16);
    let mut p = |_: Progress| {};
    let task = tokio::spawn(async move {
        // S con bloque tipo 1 y sin compresión, como en `kermit_hostile_names_and_limits`.
        let s = [
            b' ' + 80,
            b' ' + 5,
            b' ',
            0x40,
            b' ' + 13,
            b'#',
            b'N',
            b'1',
            b' ',
        ];
        let script: Vec<(u8, u8, Vec<u8>)> = vec![
            (0, b'S', s.to_vec()),
            (1, b'F', b"a".to_vec()),
            (2, b'D', b"xyz".to_vec()),
            // el emisor cambia de fichero sin cerrar el anterior con Z
            (3, b'F', b"b".to_vec()),
            (4, b'D', b"uvw".to_vec()),
            (5, b'Z', vec![]),
            (6, b'B', vec![]),
        ];
        for (seq, t, d) in script {
            a.write_all(&kpkt(seq, t, &d)).await.unwrap();
            let mut buf = [0u8; 256];
            let _ = tokio::time::timeout(Duration::from_millis(500), a.read(&mut buf)).await;
        }
    });
    let (got, _) = kermit::receive(&mut b, &dst, &mut p, u64::MAX, kcfg())
        .await
        .unwrap();
    task.await.unwrap();
    assert_eq!(got.len(), 1);
    assert_eq!(std::fs::read(&got[0]).unwrap(), b"uvw");
    assert_eq!(std::fs::read_dir(&dst).unwrap().count(), 1);
}

#[tokio::test]
async fn kermit_hostile_names_and_limits() {
    let dst = tmpdir("khost");
    let (mut a, mut b) = duplex(1 << 16);
    let mut p = |_: Progress| {};
    let task = tokio::spawn(async move {
        // emisor manual con bloque tipo 1 y sin compresión
        let s = [
            b' ' + 80,
            b' ' + 5,
            b' ',
            0x40,
            b' ' + 13,
            b'#',
            b'N',
            b'1',
            b' ',
        ];
        let script: Vec<(u8, u8, Vec<u8>)> = vec![
            (0, b'S', s.to_vec()),
            (1, b'F', b"../../etc/pass#@wd".to_vec()),
            (2, b'D', b"12345678901".to_vec()),
            (3, b'Z', vec![]),
            (4, b'B', vec![]),
        ];
        for (seq, t, d) in script {
            a.write_all(&kpkt(seq, t, &d)).await.unwrap();
            let mut buf = [0u8; 256];
            let _ = tokio::time::timeout(Duration::from_millis(500), a.read(&mut buf)).await;
        }
    });
    let (got, _) = kermit::receive(&mut b, &dst, &mut p, u64::MAX, kcfg())
        .await
        .unwrap();
    task.await.unwrap();
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].parent().unwrap(), dst);
    assert!(!got[0].file_name().unwrap().to_string_lossy().contains(".."));
    // tope total de bytes: se corta y no queda parcial
    let src = tmpdir("klim");
    let f = src.join("big");
    std::fs::write(&f, vec![7u8; 1000]).unwrap();
    let dst2 = tmpdir("klim-dst");
    let fl = [f];
    let (mut a, mut b) = duplex(1 << 16);
    let (mut p1, mut p2) = (|_: Progress| {}, |_: Progress| {});
    let (_, r) = tokio::join!(
        kermit::send(&mut a, &fl, &mut p1, kcfg()),
        kermit::receive(&mut b, &dst2, &mut p2, 100, kcfg())
    );
    assert_eq!(r.unwrap_err().kind(), io::ErrorKind::QuotaExceeded);
    assert_eq!(std::fs::read_dir(&dst2).unwrap().count(), 0);
}

/// El lector y el decodificador no entran en pánico con basura arbitraria.
#[tokio::test]
async fn kermit_garbage_never_panics() {
    let mut x = 0x1234_5678u32;
    let mut rnd = move || {
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        x
    };
    let dst = tmpdir("kgarb");
    for i in 0..300 {
        let n = (rnd() % 400) as usize;
        let bytes: Vec<u8> = (0..n)
            .map(|_| match rnd() % 8 {
                0 => 1,
                1 => b'S',
                _ => (rnd() >> 8) as u8,
            })
            .collect();
        kermit::test_decode(&bytes, i);
        let (mut a, mut b) = duplex(4096);
        a.write_all(&bytes).await.unwrap();
        drop(a);
        let mut p = |_: Progress| {};
        let _ = kermit::receive(&mut b, &dst, &mut p, 1 << 20, kcfg()).await;
    }
}

/// Proceso C-Kermit con su E/S puenteada a un duplex. C-Kermit abre `/dev/tty` siempre y
/// sin terminal de control sale con «/dev/tty is not a terminal device», de ahí el pty.
struct Kid {
    child: Box<dyn portable_pty::Child + Send + Sync>,
    _master: Box<dyn portable_pty::MasterPty + Send>,
}

impl Drop for Kid {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}

fn spawn_kermit(args: &[&str], dir: &Path) -> (Kid, DuplexStream) {
    use portable_pty::{CommandBuilder, PtySize, native_pty_system};
    let pair = native_pty_system().openpty(PtySize::default()).unwrap();
    let mut cmd = CommandBuilder::new("kermit");
    cmd.args(["-Y", "-i"]);
    cmd.args(args);
    cmd.cwd(dir);
    let child = pair.slave.spawn_command(cmd).unwrap();
    drop(pair.slave);
    let mut rd = pair.master.try_clone_reader().unwrap();
    let mut wr = pair.master.take_writer().unwrap();
    let (ours, theirs) = duplex(1 << 16);
    let (mut tr, mut tw) = tokio::io::split(theirs);
    // el pty es bloqueante: se lee/escribe en hilos y se pasa por canales
    let (up_tx, mut up_rx) = tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();
    let (down_tx, down_rx) = tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();
    tokio::task::spawn_blocking(move || {
        let mut buf = [0u8; 4096];
        while let Ok(n) = std::io::Read::read(&mut rd, &mut buf) {
            if n == 0 || up_tx.send(buf[..n].to_vec()).is_err() {
                break;
            }
        }
    });
    tokio::spawn(async move {
        while let Some(c) = up_rx.recv().await {
            if tw.write_all(&c).await.is_err() {
                break;
            }
        }
    });
    tokio::spawn(async move {
        let mut buf = [0u8; 4096];
        while let Ok(n @ 1..) = tr.read(&mut buf).await {
            if down_tx.send(buf[..n].to_vec()).is_err() {
                break;
            }
        }
    });
    let mut down_rx = down_rx;
    tokio::task::spawn_blocking(move || {
        while let Some(c) = down_rx.blocking_recv() {
            if std::io::Write::write_all(&mut wr, &c).is_err() {
                break;
            }
        }
    });
    (
        Kid {
            child,
            _master: pair.master,
        },
        ours,
    )
}

/// Interoperabilidad con C-Kermit: `FRIKI_KERMIT_INTEROP=1 cargo test -- --ignored kermit`.
#[tokio::test]
#[ignore = "requiere c-kermit instalado"]
async fn kermit_interop_ckermit() {
    if std::env::var_os("FRIKI_KERMIT_INTEROP").is_none() {
        return;
    }
    let (src, dst) = (tmpdir("kisrc"), tmpdir("kidst"));
    let data = kdata();
    let f = src.join("i.bin");
    std::fs::write(&f, &data).unwrap();
    let mut p = |_: Progress| {};
    // nosotros enviamos, C-Kermit recibe. Su `zchko` falla con un fichero nuevo («Write
    // access denied», ENOENT en open), así que se crea antes y `-w` lo sobrescribe.
    std::fs::write(dst.join("i.bin"), b"").unwrap();
    let (_c, mut io) = spawn_kermit(&["-r", "-w"], &dst);
    send(
        &mut io,
        FileProtocol::Kermit,
        std::slice::from_ref(&f),
        &mut p,
    )
    .await
    .unwrap();
    assert_eq!(std::fs::read(dst.join("i.bin")).unwrap(), data);
    // C-Kermit envía, nosotros recibimos
    let dst2 = tmpdir("kidst2");
    let (_c, mut io) = spawn_kermit(&["-s", f.to_str().unwrap()], &src);
    let r = receive_ex(&mut io, FileProtocol::Kermit, &dst2, &mut p, u64::MAX)
        .await
        .unwrap();
    assert_eq!(std::fs::read(&r.files[0]).unwrap(), data);
}
