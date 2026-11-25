//! Kermit básico (Kermit Protocol Manual, F. da Cruz): paquetes S/F/A/D/Z/B/E/Y/N, bloque de
//! control de tipo 1 y 3, prefijo de control, prefijo de 8.º bit y compresión de repeticiones.
//! Parada y espera (sin ventanas ni paquetes largos). Detección automática: ninguna, la
//! transferencia se lanza desde la TUI (la firma `SOH .. S` no es lo bastante fiable).

use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt};

use crate::{
    Conn, MAX_FILES, Progress, aborted, announce_name, create_unique, proto, read_chunk,
    sanitize_name, too_large,
};

const MARK: u8 = 1;
const MAXL: u8 = 94;

#[derive(Clone, Copy)]
pub(crate) struct Cfg {
    pub timeout: Duration,
    pub retries: u32,
    /// Proponer CRC-16 (tipo 3).
    pub chk3: bool,
    /// Proponer compresión de repeticiones (`~`).
    pub rept: bool,
    /// Propuesta de prefijo de 8.º bit: `Y` (acepto), `N` o un carácter imprimible.
    pub qbin: u8,
}

impl Default for Cfg {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(5),
            retries: 10,
            chk3: true,
            rept: true,
            qbin: b'Y',
        }
    }
}

fn tochar(n: u8) -> u8 {
    n.wrapping_add(32)
}
fn unchar(c: u8) -> u8 {
    c.wrapping_sub(32)
}
fn is_prefix(c: u8) -> bool {
    matches!(c, 33..=62 | 96..=126)
}

/// Parámetros acordados para lo que se envía/recibe.
#[derive(Clone)]
struct Link {
    chk: u8,
    maxl: usize,
    qout: u8,
    qin: u8,
    qbin: Option<u8>,
    rept: Option<u8>,
    npad: usize,
    padc: u8,
    eol: u8,
}

impl Link {
    fn initial() -> Self {
        Self {
            chk: 1,
            maxl: 80,
            qout: b'#',
            qin: b'#',
            qbin: None,
            rept: None,
            npad: 0,
            padc: 0,
            eol: 13,
        }
    }
}

/// Campos del paquete S / su ACK, en el orden del manual (hasta REPT).
fn params(cfg: &Cfg) -> Vec<u8> {
    vec![
        tochar(MAXL),
        tochar(cfg.timeout.as_secs().min(90) as u8),
        tochar(0),
        0x40, // ctl(NUL)
        tochar(13),
        b'#',
        cfg.qbin,
        if cfg.chk3 { b'3' } else { b'1' },
        if cfg.rept { b'~' } else { b' ' },
    ]
}

fn negotiate(mine: &[u8], theirs: &[u8]) -> Link {
    let g = |d: &[u8], i: usize, def: u8| d.get(i).copied().unwrap_or(def);
    let qin = Some(g(theirs, 5, b'#'))
        .filter(|&q| is_prefix(q))
        .unwrap_or(b'#');
    let qout = Some(g(mine, 5, b'#'))
        .filter(|&q| is_prefix(q))
        .unwrap_or(b'#');
    let rept = {
        let (m, t) = (g(mine, 8, b' '), g(theirs, 8, b' '));
        (m == t && is_prefix(m) && m != qin && m != qout).then_some(m)
    };
    let qbin = {
        let (m, t) = (g(mine, 6, b'N'), g(theirs, 6, b'N'));
        let q = if is_prefix(m) && (t == b'Y' || t == m) {
            Some(m)
        } else if is_prefix(t) && m == b'Y' {
            Some(t)
        } else {
            None
        };
        q.filter(|&q| q != qin && q != qout && Some(q) != rept)
    };
    let eol = match unchar(g(theirs, 4, tochar(13))) {
        e @ 1..=31 => e,
        _ => 13,
    };
    Link {
        chk: if g(mine, 7, b'1') == b'3' && g(theirs, 7, b'1') == b'3' {
            3
        } else {
            1
        },
        maxl: match unchar(g(theirs, 0, tochar(80))) {
            n @ 10..=MAXL => usize::from(n),
            _ => 80,
        },
        qout,
        qin,
        qbin,
        rept,
        npad: usize::from(unchar(g(theirs, 2, tochar(0))).min(10)),
        padc: g(theirs, 3, 0x40) ^ 0x40,
        eol,
    }
}

fn check(chk: u8, body: &[u8]) -> Vec<u8> {
    if chk == 3 {
        // CRC-16 Kermit: polinomio reflejado 0x8408, inicial 0.
        let mut crc = 0u16;
        for &b in body {
            crc ^= u16::from(b);
            for _ in 0..8 {
                crc = if crc & 1 != 0 {
                    (crc >> 1) ^ 0x8408
                } else {
                    crc >> 1
                };
            }
        }
        vec![
            tochar(((crc >> 12) & 0xf) as u8),
            tochar(((crc >> 6) & 0x3f) as u8),
            tochar((crc & 0x3f) as u8),
        ]
    } else {
        let s: u32 = body.iter().map(|&b| u32::from(b)).sum();
        vec![tochar(((s + ((s & 0xc0) >> 6)) & 0x3f) as u8)]
    }
}

fn packet(l: &Link, seq: u8, typ: u8, data: &[u8]) -> Vec<u8> {
    let ck = if l.chk == 3 { 3 } else { 1 };
    let mut body = vec![tochar((2 + data.len() + ck) as u8), tochar(seq), typ];
    body.extend_from_slice(data);
    let mut out = vec![l.padc; l.npad];
    out.push(MARK);
    out.extend_from_slice(&body);
    out.extend(check(l.chk, &body));
    out.push(l.eol);
    out
}

enum Rx {
    Pkt(u8, u8, Vec<u8>),
    Bad,
}

/// Lee un paquete. Tres Ctrl+C seguidos del remoto abortan; un byte SOH dentro de un
/// paquete lo reinicia. Nunca entra en pánico con entrada arbitraria.
async fn read_packet<S: AsyncRead + AsyncWrite + Unpin + Send>(
    c: &mut Conn<'_, S>,
    chk: u8,
    t: Duration,
) -> io::Result<Rx> {
    let mut ctrlc = 0;
    loop {
        match c.byte(t).await? {
            MARK => break,
            3 => {
                ctrlc += 1;
                if ctrlc >= 3 {
                    return Err(aborted());
                }
            }
            _ => ctrlc = 0,
        }
    }
    let ck = if chk == 3 { 3 } else { 1 };
    let (l, body) = 'len: loop {
        let l = c.byte(t).await?;
        if l == MARK {
            continue;
        }
        let n = usize::from(unchar(l));
        if !(32..=126).contains(&l) || n < 2 + ck {
            return Ok(Rx::Bad); // incluye LEN=0 (paquete largo, no negociado)
        }
        let mut body = Vec::with_capacity(n);
        for _ in 0..n {
            let b = c.byte(t).await?;
            if b == MARK {
                continue 'len;
            }
            body.push(b);
        }
        break (l, body);
    };
    let (head, tail) = body.split_at(body.len() - ck);
    let mut summed = vec![l];
    summed.extend_from_slice(head);
    let seq = unchar(head[0]);
    if check(chk, &summed) != tail || seq > 63 || !(32..127).contains(&head[1]) {
        return Ok(Rx::Bad);
    }
    Ok(Rx::Pkt(seq, head[1], head[2..].to_vec()))
}

fn encode_item(b: u8, l: &Link, out: &mut Vec<u8>) {
    let mut a = b;
    if b & 0x80 != 0
        && let Some(q) = l.qbin
    {
        out.push(q);
        a = b & 0x7f;
    }
    if a < 32 || a == 127 {
        out.extend([l.qout, a ^ 0x40]);
    } else if a == l.qout || Some(a) == l.qbin || Some(a) == l.rept {
        out.extend([l.qout, a]);
    } else {
        out.push(a);
    }
}

/// Codifica como mucho `limit` bytes de `buf`; devuelve (datos, bytes consumidos).
fn encode_data(buf: &[u8], l: &Link, limit: usize) -> (Vec<u8>, usize) {
    let (mut out, mut i) = (Vec::new(), 0);
    while i < buf.len() {
        let b = buf[i];
        let mut item = Vec::new();
        let mut n = 1;
        if let Some(r) = l.rept {
            let run = buf[i..].iter().take(94).take_while(|&&x| x == b).count();
            if run >= 4 {
                item.extend([r, tochar(run as u8)]);
                n = run;
            }
        }
        encode_item(b, l, &mut item);
        if out.len() + item.len() > limit {
            break;
        }
        out.extend(item);
        i += n;
    }
    (out, i)
}

/// Inverso de `encode_data`; `None` si el campo está truncado o es incoherente.
fn decode(data: &[u8], l: &Link) -> Option<Vec<u8>> {
    let (mut out, mut i) = (Vec::new(), 0);
    while i < data.len() {
        let mut count = 1;
        if Some(data[i]) == l.rept {
            count = usize::from(unchar(*data.get(i + 1)?));
            if count > 94 {
                return None;
            }
            i += 2;
        }
        let mut hi = false;
        if Some(*data.get(i)?) == l.qbin {
            hi = true;
            i += 1;
        }
        let mut c = *data.get(i)?;
        i += 1;
        if c == l.qin {
            let n = *data.get(i)?;
            i += 1;
            // literal si su parte de 7 bits es QCTL/QBIN/REPT (C-Kermit sin QBIN envía así
            // p. ej. 0xA3 como `#` 0xA3); si no, control con el bit 6 invertido
            let lit = |x: u8| {
                let x = x & 0x7f;
                x == l.qin || Some(x) == l.qbin || Some(x) == l.rept
            };
            c = if lit(n) { n } else { n ^ 0x40 };
        }
        if hi {
            c |= 0x80;
        }
        out.extend(std::iter::repeat_n(c, count));
    }
    Some(out)
}

fn remote_err(d: &[u8]) -> io::Error {
    let msg: String = d
        .iter()
        .filter(|b| (32..127).contains(*b))
        .take(80)
        .map(|&b| char::from(b))
        .collect();
    io::Error::new(
        io::ErrorKind::ConnectionAborted,
        format!("error del remoto: {msg}"),
    )
}

fn timed_out() -> io::Error {
    io::Error::new(io::ErrorKind::TimedOut, "el remoto no responde")
}

struct Session<'a, S> {
    c: Conn<'a, S>,
    link: Link,
    cfg: Cfg,
    seq: u8,
}

impl<S: AsyncRead + AsyncWrite + Unpin + Send> Session<'_, S> {
    /// Envía un paquete y espera su ACK (reintentos acotados); devuelve los datos del ACK.
    async fn exchange(&mut self, typ: u8, data: &[u8]) -> io::Result<Vec<u8>> {
        let pkt = packet(&self.link, self.seq, typ, data);
        self.c.write(&pkt).await?;
        let mut tries = 0;
        let next = (self.seq + 1) & 63;
        loop {
            let resend = match read_packet(&mut self.c, self.link.chk, self.cfg.timeout).await {
                Ok(Rx::Pkt(_, b'E', d)) => return Err(remote_err(&d)),
                Ok(Rx::Pkt(s, b'Y', d)) if s == self.seq => {
                    self.seq = next;
                    return Ok(d);
                }
                Ok(Rx::Pkt(s, b'N', _)) if s == next => {
                    self.seq = next; // el NAK del siguiente equivale a un ACK
                    return Ok(Vec::new());
                }
                Ok(Rx::Pkt(_, b'N', _) | Rx::Bad) => true,
                Ok(Rx::Pkt(..)) => false, // duplicado viejo: se ignora
                Err(e) if e.kind() == io::ErrorKind::TimedOut => true,
                Err(e) => return Err(e),
            };
            tries += 1;
            if tries > self.cfg.retries {
                return Err(timed_out());
            }
            if resend {
                self.c.write(&pkt).await?;
            }
        }
    }

    /// Paquete de error de cortesía; los fallos de escritura se ignoran.
    async fn fail(&mut self, e: &io::Error) {
        let msg: String = e
            .to_string()
            .chars()
            .map(|c| {
                if c.is_ascii_graphic() || c == ' ' {
                    c
                } else {
                    '?'
                }
            })
            .take(60)
            .collect();
        let p = packet(&self.link, self.seq, b'E', msg.as_bytes());
        let _ = self.c.write(&p).await;
    }
}

pub(crate) async fn send<S: AsyncRead + AsyncWrite + Unpin + Send>(
    io: &mut S,
    files: &[PathBuf],
    progress: &mut (dyn FnMut(Progress) + Send),
    cfg: Cfg,
) -> io::Result<()> {
    let mut s = Session {
        c: Conn::new(io),
        link: Link::initial(),
        cfg,
        seq: 0,
    };
    let r = send_inner(&mut s, files, progress).await;
    if let Err(e) = &r
        && e.kind() != io::ErrorKind::ConnectionAborted
    {
        s.fail(e).await;
    }
    r
}

async fn send_inner<S: AsyncRead + AsyncWrite + Unpin + Send>(
    s: &mut Session<'_, S>,
    files: &[PathBuf],
    progress: &mut (dyn FnMut(Progress) + Send),
) -> io::Result<()> {
    let mine = params(&s.cfg);
    let ack = s.exchange(b'S', &mine).await?;
    s.link = negotiate(&mine, &ack);
    for path in files {
        let name = announce_name(path);
        let mut f = tokio::fs::File::open(path).await?;
        let total = f.metadata().await?.len();
        let limit = s.link.maxl - 2 - if s.link.chk == 3 { 3 } else { 1 };
        // ponytail: nombre truncado al tamaño de un paquete, sin cabecera A de atributos
        let reply = s
            .exchange(b'F', &encode_data(name.as_bytes(), &s.link, limit).0)
            .await?;
        let skip = matches!(reply.as_slice(), b"X" | b"Z");
        let mut stop = reply == b"Z";
        let (mut buf, mut off, mut done, mut eof) = (Vec::new(), 0usize, 0u64, false);
        loop {
            if skip {
                break;
            }
            if buf.len() - off < 256 && !eof {
                buf.drain(..off);
                off = 0;
                let more = read_chunk(&mut f, 4096).await?;
                eof = more.is_empty();
                buf.extend(more);
            }
            if off == buf.len() {
                break;
            }
            let (data, n) = encode_data(&buf[off..], &s.link, limit);
            let reply = s.exchange(b'D', &data).await?;
            off += n;
            done += n as u64;
            progress(Progress {
                file: name.clone(),
                done,
                total: Some(total),
            });
            if matches!(reply.as_slice(), b"X" | b"Z") {
                stop |= reply == b"Z";
                break; // el receptor pide interrumpir este fichero
            }
        }
        s.exchange(b'Z', if skip { b"D" } else { b"" }).await?;
        if stop {
            break;
        }
    }
    s.exchange(b'B', b"").await?;
    Ok(())
}

struct Cur {
    file: tokio::fs::File,
    path: PathBuf,
    name: String,
    written: u64,
    total: Option<u64>,
}

pub(crate) async fn receive<S: AsyncRead + AsyncWrite + Unpin + Send>(
    io: &mut S,
    dest: &Path,
    progress: &mut (dyn FnMut(Progress) + Send),
    max_total: u64,
    cfg: Cfg,
) -> io::Result<(Vec<PathBuf>, Vec<u8>)> {
    let mut s = Session {
        c: Conn::new(io),
        link: Link::initial(),
        cfg,
        seq: 0, // = siguiente paquete esperado
    };
    let mut out = Vec::new();
    let mut cur = None;
    match recv_inner(&mut s, dest, progress, max_total, &mut out, &mut cur).await {
        Ok(()) => Ok((out, s.c.rest())),
        Err(e) => {
            if let Some(k) = cur.take() {
                drop(k.file);
                let _ = tokio::fs::remove_file(&k.path).await;
            }
            if e.kind() != io::ErrorKind::ConnectionAborted {
                s.fail(&e).await;
            }
            Err(e)
        }
    }
}

async fn recv_inner<S: AsyncRead + AsyncWrite + Unpin + Send>(
    s: &mut Session<'_, S>,
    dest: &Path,
    progress: &mut (dyn FnMut(Progress) + Send),
    max_total: u64,
    out: &mut Vec<PathBuf>,
    cur: &mut Option<Cur>,
) -> io::Result<()> {
    let mut started = false;
    let mut last: Vec<u8> = Vec::new(); // último ACK, por si llega duplicado
    let mut done_total = 0u64;
    let mut errs = 0;
    loop {
        let rx = read_packet(&mut s.c, s.link.chk, s.cfg.timeout).await;
        let (seq, typ, data) = match rx {
            Ok(Rx::Pkt(seq, typ, data)) => (seq, typ, data),
            Ok(Rx::Bad) => (u8::MAX, 0, Vec::new()),
            Err(e) if e.kind() == io::ErrorKind::TimedOut => (u8::MAX, 0, Vec::new()),
            Err(e) => return Err(e),
        };
        if typ == b'E' {
            return Err(remote_err(&data));
        }
        let dup = started && seq == (s.seq + 63) & 63;
        if seq != s.seq {
            errs += 1;
            if errs > s.cfg.retries {
                return Err(timed_out());
            }
            // duplicado: se repite el ACK; si no, NAK del esperado (también anuncia al emisor)
            let p = if dup && !last.is_empty() {
                last.clone()
            } else {
                packet(&s.link, s.seq, b'N', b"")
            };
            s.c.write(&p).await?;
            continue;
        }
        errs = 0;
        let mut reply = Vec::new();
        match typ {
            b'S' if !started => {
                let next = negotiate(&params(&s.cfg), &data);
                reply = params(&Cfg {
                    chk3: next.chk == 3,
                    ..s.cfg
                });
                last = packet(&s.link, seq, b'Y', &reply); // el ACK de S va con bloque tipo 1
                s.c.write(&last).await?;
                s.link = next;
                started = true;
                s.seq = (s.seq + 1) & 63;
                continue;
            }
            b'F' if started => {
                // Sin Z, el fichero anterior quedó a medias: se borra y no cuenta como recibido.
                if let Some(k) = cur.take() {
                    drop(k.file);
                    let _ = tokio::fs::remove_file(&k.path).await;
                    out.pop();
                }
                if out.len() >= MAX_FILES {
                    return Err(proto("demasiados ficheros en el lote"));
                }
                let raw = decode(&data, &s.link).ok_or_else(|| proto("nombre corrupto"))?;
                let name = sanitize_name(&raw);
                let (file, path) = create_unique(dest, &name).await?;
                out.push(path.clone());
                *cur = Some(Cur {
                    file,
                    path,
                    name,
                    written: 0,
                    total: None,
                });
            }
            b'A' if cur.is_some() => {
                // atributos: solo se lee la longitud en bytes ('!') para acotar el fichero
                let d = decode(&data, &s.link).unwrap_or_default();
                let mut i = 0;
                while let (Some(&tag), Some(&l)) = (d.get(i), d.get(i + 1)) {
                    let end = (i + 2 + usize::from(unchar(l))).min(d.len());
                    if tag == b'!'
                        && let Some(k) = cur.as_mut()
                    {
                        k.total = std::str::from_utf8(&d[i + 2..end])
                            .ok()
                            .and_then(|v| v.parse().ok());
                    }
                    i = end;
                }
            }
            b'D' if cur.is_some() => {
                let Some(bytes) = decode(&data, &s.link) else {
                    s.c.write(&packet(&s.link, s.seq, b'N', b"")).await?;
                    continue;
                };
                let Some(k) = cur.as_mut() else { continue };
                let after = k.written + bytes.len() as u64;
                if k.total.is_some_and(|t| after > t) {
                    return Err(too_large("el fichero supera el tamaño anunciado"));
                }
                if done_total.saturating_add(after) > max_total {
                    return Err(too_large("se supera el tope total de bytes"));
                }
                k.file.write_all(&bytes).await?;
                k.written = after;
                progress(Progress {
                    file: k.name.clone(),
                    done: after,
                    total: k.total,
                });
            }
            b'Z' if cur.is_some() => {
                if let Some(mut k) = cur.take() {
                    if data.first() == Some(&b'D') {
                        drop(k.file);
                        let _ = tokio::fs::remove_file(&k.path).await;
                        out.pop();
                    } else {
                        k.file.flush().await?;
                        done_total = done_total.saturating_add(k.written);
                    }
                }
            }
            b'B' if started => {
                s.c.write(&packet(&s.link, seq, b'Y', b"")).await?;
                return Ok(());
            }
            _ => return Err(proto("paquete Kermit inesperado")),
        }
        last = packet(&s.link, seq, b'Y', &reply);
        s.c.write(&last).await?;
        s.seq = (s.seq + 1) & 63;
    }
}

#[cfg(test)]
pub(crate) fn test_packet(seq: u8, typ: u8, data: &[u8]) -> Vec<u8> {
    packet(&Link::initial(), seq, typ, data)
}

#[cfg(test)]
#[test]
fn decode_literal_8bit_after_qctl() {
    let l = Link {
        rept: Some(b'~'),
        ..Link::initial()
    };
    assert_eq!(decode(b"#\xa3", &l), Some(vec![0xa3]));
    assert_eq!(decode(b"#\xfe", &l), Some(vec![0xfe]));
    assert_eq!(decode(b"#\xdd", &l), Some(vec![0x9d]));
}

/// Decodifica basura con enlaces variados (prueba de no-pánico).
#[cfg(test)]
pub(crate) fn test_decode(data: &[u8], i: u32) {
    let mut l = Link::initial();
    l.qbin = i.is_multiple_of(2).then_some(b'&');
    l.rept = i.is_multiple_of(3).then_some(b'~');
    let _ = decode(data, &l);
}
