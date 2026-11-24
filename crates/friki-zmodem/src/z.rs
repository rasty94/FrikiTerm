//! Zmodem: cabeceras hex/binarias con CRC-16 y CRC-32, escapado ZDLE, ventanas de
//! subpaquetes con ZACK (sin streaming completo) y recuperación con ZRPOS.

use std::io::{self, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncSeekExt, AsyncWrite, AsyncWriteExt};

use crate::{
    Conn, MAX_FILES, Progress, T, aborted, announce_name, crc16_update, crc32_update,
    create_unique, proto, read_chunk, sanitize_name,
};

const ZPAD: u8 = b'*';
const ZDLE: u8 = 0x18;
const ZBIN: u8 = b'A';
const ZHEX: u8 = b'B';
const ZBIN32: u8 = b'C';

pub(crate) const ZRQINIT: u8 = 0;
pub(crate) const ZRINIT: u8 = 1;
const ZSINIT: u8 = 2;
const ZACK: u8 = 3;
pub(crate) const ZFILE: u8 = 4;
const ZSKIP: u8 = 5;
const ZABORT: u8 = 7;
const ZFIN: u8 = 8;
const ZRPOS: u8 = 9;
pub(crate) const ZDATA: u8 = 10;
const ZEOF: u8 = 11;
const ZCAN: u8 = 16;

const ZCRCE: u8 = b'h';
const ZCRCG: u8 = b'i';
const ZCRCQ: u8 = b'j';
const ZCRCW: u8 = b'k';

const CANFDX: u8 = 0x01;
const CANOVIO: u8 = 0x02;
const CANFC32: u8 = 0x20;

const SUB_LEN: usize = 1024;
const WINDOW: usize = 8;
const MAX_SUB: usize = 8192;
const RETRIES: u32 = 5;

#[derive(Debug, Clone, Copy)]
pub(crate) struct Hdr {
    pub typ: u8,
    pub a: [u8; 4],
    pub c32: bool,
}

impl Hdr {
    pub(crate) fn pos(typ: u8, pos: u64) -> Self {
        Self {
            typ,
            a: (pos as u32).to_le_bytes(),
            c32: false,
        }
    }
    pub(crate) fn flags(typ: u8, zf0: u8) -> Self {
        Self {
            typ,
            a: [0, 0, 0, zf0],
            c32: false,
        }
    }
    fn position(&self) -> u32 {
        u32::from_le_bytes(self.a)
    }
    fn body(&self) -> [u8; 5] {
        [self.typ, self.a[0], self.a[1], self.a[2], self.a[3]]
    }
}

fn put_esc(out: &mut Vec<u8>, b: u8) {
    match b {
        0x18 | 0x10 | 0x90 | 0x11 | 0x91 | 0x13 | 0x93 => out.extend([ZDLE, b ^ 0x40]),
        _ => out.push(b),
    }
}

fn crc32_of(data: &[u8]) -> u32 {
    !crc32_update(!0, data)
}

pub(crate) fn hex_header(h: &Hdr) -> Vec<u8> {
    let mut raw = h.body().to_vec();
    raw.extend(crc16_update(0, &raw).to_be_bytes());
    let mut out = vec![ZPAD, ZPAD, ZDLE, ZHEX];
    for b in raw {
        out.extend(format!("{b:02x}").bytes());
    }
    out.extend([0x0d, 0x8a]);
    if h.typ != ZFIN && h.typ != ZACK {
        out.push(0x11);
    }
    out
}

pub(crate) fn bin_header(h: &Hdr, c32: bool) -> Vec<u8> {
    let mut out = vec![ZPAD, ZDLE, if c32 { ZBIN32 } else { ZBIN }];
    let body = h.body();
    for b in body {
        put_esc(&mut out, b);
    }
    if c32 {
        for b in crc32_of(&body).to_le_bytes() {
            put_esc(&mut out, b);
        }
    } else {
        for b in crc16_update(0, &body).to_be_bytes() {
            put_esc(&mut out, b);
        }
    }
    out
}

pub(crate) fn data_subpacket(data: &[u8], term: u8, c32: bool) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len() + 16);
    for &b in data {
        put_esc(&mut out, b);
    }
    out.extend([ZDLE, term]);
    if c32 {
        let crc = !crc32_update(crc32_update(!0, data), &[term]);
        for b in crc.to_le_bytes() {
            put_esc(&mut out, b);
        }
    } else {
        let crc = crc16_update(crc16_update(0, data), &[term]);
        for b in crc.to_be_bytes() {
            put_esc(&mut out, b);
        }
    }
    if term == ZCRCW {
        out.push(0x11);
    }
    out
}

enum Esc {
    Byte(u8),
    Term(u8),
}

async fn read_esc<S: AsyncRead + AsyncWrite + Unpin + Send>(
    c: &mut Conn<'_, S>,
) -> io::Result<Esc> {
    loop {
        let b = c.byte(T).await?;
        match b {
            0x11 | 0x13 | 0x91 | 0x93 => continue,
            ZDLE => {
                let n = c.byte(T).await?;
                return match n {
                    ZCRCE | ZCRCG | ZCRCQ | ZCRCW => Ok(Esc::Term(n)),
                    b'l' => Ok(Esc::Byte(0x7f)),
                    b'm' => Ok(Esc::Byte(0xff)),
                    ZDLE => Err(aborted()),
                    _ => Ok(Esc::Byte(n ^ 0x40)),
                };
            }
            _ => return Ok(Esc::Byte(b)),
        }
    }
}

fn hexval(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// Busca la siguiente cabecera válida; descarta basura y cabeceras con CRC erróneo.
async fn read_header<S: AsyncRead + AsyncWrite + Unpin + Send>(
    c: &mut Conn<'_, S>,
) -> io::Result<Hdr> {
    let mut cans = 0;
    loop {
        let b = c.byte(T).await?;
        if b == ZDLE {
            cans += 1;
            if cans >= 5 {
                return Err(aborted());
            }
        } else {
            cans = 0;
        }
        if b != ZPAD {
            continue;
        }
        let mut n = c.byte(T).await?;
        while n == ZPAD {
            n = c.byte(T).await?;
        }
        if n != ZDLE {
            continue;
        }
        let fmt = c.byte(T).await?;
        match fmt {
            ZHEX => {
                let mut raw = [0u8; 7];
                let mut ok = true;
                for r in raw.iter_mut() {
                    let hi = hexval(c.byte(T).await?);
                    let lo = hexval(c.byte(T).await?);
                    match (hi, lo) {
                        (Some(h), Some(l)) => *r = h << 4 | l,
                        _ => ok = false,
                    }
                }
                if ok && crc16_update(0, &raw[..5]).to_be_bytes() == [raw[5], raw[6]] {
                    return Ok(Hdr {
                        typ: raw[0],
                        a: [raw[1], raw[2], raw[3], raw[4]],
                        c32: false,
                    });
                }
            }
            ZBIN | ZBIN32 => {
                let c32 = fmt == ZBIN32;
                let total = if c32 { 9 } else { 7 };
                let mut raw = Vec::with_capacity(total);
                let mut ok = true;
                for _ in 0..total {
                    match read_esc(c).await? {
                        Esc::Byte(v) => raw.push(v),
                        Esc::Term(_) => {
                            ok = false;
                            break;
                        }
                    }
                }
                if ok {
                    let good = if c32 {
                        crc32_of(&raw[..5]).to_le_bytes() == raw[5..9]
                    } else {
                        crc16_update(0, &raw[..5]).to_be_bytes() == raw[5..7]
                    };
                    if good {
                        return Ok(Hdr {
                            typ: raw[0],
                            a: [raw[1], raw[2], raw[3], raw[4]],
                            c32,
                        });
                    }
                }
            }
            _ => {}
        }
    }
}

enum Sub {
    Ok(Vec<u8>, u8),
    Bad,
}

async fn read_subpacket<S: AsyncRead + AsyncWrite + Unpin + Send>(
    c: &mut Conn<'_, S>,
    c32: bool,
) -> io::Result<Sub> {
    let mut data = Vec::new();
    let term = loop {
        match read_esc(c).await? {
            Esc::Byte(b) => {
                if data.len() >= MAX_SUB {
                    return Ok(Sub::Bad);
                }
                data.push(b);
            }
            Esc::Term(t) => break t,
        }
    };
    let n = if c32 { 4 } else { 2 };
    let mut crc = Vec::with_capacity(n);
    for _ in 0..n {
        match read_esc(c).await? {
            Esc::Byte(b) => crc.push(b),
            Esc::Term(_) => return Ok(Sub::Bad),
        }
    }
    let good = if c32 {
        (!crc32_update(crc32_update(!0, &data), &[term])).to_le_bytes() == crc[..]
    } else {
        crc16_update(crc16_update(0, &data), &[term]).to_be_bytes() == crc[..]
    };
    Ok(if good { Sub::Ok(data, term) } else { Sub::Bad })
}

fn timed_out() -> io::Error {
    io::Error::new(io::ErrorKind::TimedOut, "el remoto no responde")
}

struct Cur {
    file: tokio::fs::File,
    name: String,
    written: u64,
    total: Option<u64>,
}

pub(crate) async fn receive<S: AsyncRead + AsyncWrite + Unpin + Send>(
    io: &mut S,
    dest: &Path,
    progress: &mut (dyn FnMut(Progress) + Send),
) -> io::Result<Vec<PathBuf>> {
    let mut c = Conn::new(io);
    let mut out: Vec<PathBuf> = Vec::new();
    let zrinit = hex_header(&Hdr::flags(ZRINIT, CANFDX | CANOVIO | CANFC32));
    c.write(&zrinit).await?;
    let mut last = zrinit.clone();
    let mut timeouts = 0;
    let mut cur: Option<Cur> = None;
    let mut errored = false;
    loop {
        let h = match read_header(&mut c).await {
            Ok(h) => {
                timeouts = 0;
                h
            }
            Err(e) if e.kind() == io::ErrorKind::TimedOut => {
                timeouts += 1;
                if timeouts > RETRIES {
                    return Err(e);
                }
                c.write(&last).await?;
                continue;
            }
            Err(e) => return Err(e),
        };
        match h.typ {
            ZRQINIT => c.write(&zrinit).await?,
            ZSINIT => {
                if let Sub::Ok(..) = read_subpacket(&mut c, h.c32).await? {
                    c.write(&hex_header(&Hdr::pos(ZACK, 0))).await?;
                }
            }
            ZFILE => {
                let Sub::Ok(info, _) = read_subpacket(&mut c, h.c32).await? else {
                    c.write(&hex_header(&Hdr::pos(6, 0))).await?; // ZNAK
                    continue;
                };
                if cur.as_ref().is_some_and(|k| k.written == 0) {
                    c.write(&last).await?; // ZFILE repetido
                    continue;
                }
                if out.len() >= MAX_FILES {
                    c.cancel().await;
                    return Err(proto("demasiados ficheros en el lote"));
                }
                let end = info.iter().position(|&b| b == 0).unwrap_or(info.len());
                let name = sanitize_name(&info[..end]);
                let total = info.get(end + 1..).and_then(|r| {
                    let d: Vec<u8> = r.iter().copied().take_while(u8::is_ascii_digit).collect();
                    String::from_utf8(d).ok()?.parse::<u64>().ok()
                });
                let (file, path) = create_unique(dest, &name).await?;
                out.push(path);
                cur = Some(Cur {
                    file,
                    name,
                    written: 0,
                    total,
                });
                errored = false;
                last = hex_header(&Hdr::pos(ZRPOS, 0));
                c.write(&last).await?;
            }
            ZDATA => {
                let Some(k) = cur.as_mut() else { continue };
                if u64::from(h.position()) != k.written & 0xffff_ffff {
                    if !errored {
                        errored = true;
                        last = hex_header(&Hdr::pos(ZRPOS, k.written));
                        c.write(&last).await?;
                    }
                    continue;
                }
                errored = false;
                loop {
                    match read_subpacket(&mut c, h.c32).await? {
                        Sub::Ok(data, term) => {
                            k.file.write_all(&data).await?;
                            k.written += data.len() as u64;
                            progress(Progress {
                                file: k.name.clone(),
                                done: k.written,
                                total: k.total,
                            });
                            if term == ZCRCW || term == ZCRCQ {
                                last = hex_header(&Hdr::pos(ZACK, k.written));
                                c.write(&last).await?;
                            }
                            if term == ZCRCW || term == ZCRCE {
                                break;
                            }
                        }
                        Sub::Bad => {
                            errored = true;
                            last = hex_header(&Hdr::pos(ZRPOS, k.written));
                            c.write(&last).await?;
                            break;
                        }
                    }
                }
            }
            ZEOF => {
                let Some(k) = cur.as_mut() else { continue };
                if u64::from(h.position()) == k.written & 0xffff_ffff {
                    k.file.flush().await?;
                    cur = None;
                    errored = false;
                    last = zrinit.clone();
                    c.write(&last).await?;
                } else if !errored {
                    errored = true;
                    last = hex_header(&Hdr::pos(ZRPOS, k.written));
                    c.write(&last).await?;
                }
            }
            ZFIN => {
                c.write(&hex_header(&Hdr::pos(ZFIN, 0))).await?;
                // "OO" de cortesía; si no llega, no importa.
                let _ = c.byte(Duration::from_millis(500)).await;
                return Ok(out);
            }
            ZCAN | ZABORT => return Err(aborted()),
            _ => {}
        }
    }
}

pub(crate) async fn send<S: AsyncRead + AsyncWrite + Unpin + Send>(
    io: &mut S,
    files: &[PathBuf],
    progress: &mut (dyn FnMut(Progress) + Send),
) -> io::Result<()> {
    let mut c = Conn::new(io);
    let rq = hex_header(&Hdr::pos(ZRQINIT, 0));
    c.write(&rq).await?;
    let mut tries = 0;
    let flags = loop {
        match read_header(&mut c).await {
            Ok(h) if h.typ == ZRINIT => break h.a[3],
            Ok(h) if h.typ == ZCAN || h.typ == ZABORT => return Err(aborted()),
            Ok(_) => {}
            Err(e) if e.kind() == io::ErrorKind::TimedOut => {
                tries += 1;
                if tries > RETRIES {
                    return Err(e);
                }
                c.write(&rq).await?;
            }
            Err(e) => return Err(e),
        }
    };
    let c32 = flags & CANFC32 != 0;
    let total_left = files.len();
    'files: for (i, path) in files.iter().enumerate() {
        let mut f = tokio::fs::File::open(path).await?;
        let len = f.metadata().await?.len();
        if len > u64::from(u32::MAX) {
            return Err(crate::invalid(
                "fichero demasiado grande para Zmodem (>4 GiB)",
            ));
        }
        let name = announce_name(path);
        let mut info = name.clone().into_bytes();
        info.push(0);
        info.extend(format!("{len} 0 0 0 {} {len}", total_left - i).bytes());
        let mut zfile = bin_header(&Hdr::flags(ZFILE, 1), c32);
        zfile.extend(data_subpacket(&info, ZCRCW, c32));
        c.write(&zfile).await?;
        let mut tries = 0;
        let mut pos: u64 = loop {
            match read_header(&mut c).await {
                Ok(h) if h.typ == ZRPOS => break u64::from(h.position()),
                Ok(h) if h.typ == ZSKIP => continue 'files,
                Ok(h) if h.typ == ZCAN || h.typ == ZABORT => return Err(aborted()),
                Ok(_) => {} // ZRINIT duplicado, etc.
                Err(e) if e.kind() == io::ErrorKind::TimedOut => {
                    tries += 1;
                    if tries > RETRIES {
                        return Err(e);
                    }
                    c.write(&zfile).await?;
                }
                Err(e) => return Err(e),
            }
        };
        'data: loop {
            if pos > len {
                return Err(proto("ZRPOS fuera del fichero"));
            }
            f.seek(SeekFrom::Start(pos)).await?;
            loop {
                c.write(&bin_header(&Hdr::pos(ZDATA, pos), c32)).await?;
                let mut last = false;
                let mut pkt = Vec::new();
                for i in 0..WINDOW {
                    let chunk = read_chunk(&mut f, SUB_LEN).await?;
                    pos += chunk.len() as u64;
                    last = pos >= len;
                    let term = if last {
                        ZCRCE
                    } else if i == WINDOW - 1 {
                        ZCRCW
                    } else {
                        ZCRCG
                    };
                    pkt.extend(data_subpacket(&chunk, term, c32));
                    if last {
                        break;
                    }
                }
                if last {
                    pkt.extend(bin_header(&Hdr::pos(ZEOF, pos), c32));
                }
                c.write(&pkt).await?;
                progress(Progress {
                    file: name.clone(),
                    done: pos,
                    total: Some(len),
                });
                let mut tries = 0;
                loop {
                    match read_header(&mut c).await {
                        Ok(h) if h.typ == ZRPOS => {
                            pos = u64::from(h.position());
                            continue 'data;
                        }
                        Ok(h) if h.typ == ZRINIT && last => continue 'files,
                        Ok(h) if h.typ == ZACK && !last => break,
                        Ok(h) if h.typ == ZSKIP => continue 'files,
                        Ok(h) if h.typ == ZCAN || h.typ == ZABORT => return Err(aborted()),
                        Ok(_) => {}
                        Err(e) if e.kind() == io::ErrorKind::TimedOut => {
                            tries += 1;
                            if tries > RETRIES {
                                return Err(timed_out());
                            }
                            if last {
                                c.write(&bin_header(&Hdr::pos(ZEOF, pos), c32)).await?;
                            }
                        }
                        Err(e) => return Err(e),
                    }
                }
            }
        }
    }
    c.write(&hex_header(&Hdr::pos(ZFIN, 0))).await?;
    for _ in 0..RETRIES {
        match read_header(&mut c).await {
            Ok(h) if h.typ == ZFIN => break,
            Ok(_) => {}
            Err(_) => break,
        }
    }
    c.write(b"OO").await
}
