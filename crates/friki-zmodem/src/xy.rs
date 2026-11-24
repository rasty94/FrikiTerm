//! Xmodem (checksum y CRC, bloques de 128 y 1024) y Ymodem (bloque 0 + lote).

use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt};

use crate::{
    Conn, MAX_FILES, Progress, T, aborted, announce_name, crc16_update, create_unique, proto,
    read_chunk, sanitize_name,
};

const SOH: u8 = 0x01;
const STX: u8 = 0x02;
const EOT: u8 = 0x04;
const ACK: u8 = 0x06;
const NAK: u8 = 0x15;
const CAN: u8 = 0x18;
const SUB: u8 = 0x1a;
const START_WAIT: Duration = Duration::from_secs(60);
const PROBE_WAIT: Duration = Duration::from_secs(3);
const RETRIES: u32 = 10;

enum Blk {
    Eot,
    Data(u8, Vec<u8>),
    Bad,
}

fn is_timeout(e: &io::Error) -> bool {
    e.kind() == io::ErrorKind::TimedOut
}

async fn read_rest<S: AsyncRead + AsyncWrite + Unpin + Send>(
    c: &mut Conn<'_, S>,
    size: usize,
    crc: bool,
) -> io::Result<Blk> {
    let seq = c.byte(T).await?;
    let cseq = c.byte(T).await?;
    let mut data = Vec::with_capacity(size);
    for _ in 0..size {
        data.push(c.byte(T).await?);
    }
    let ok = if crc {
        let hi = c.byte(T).await?;
        let lo = c.byte(T).await?;
        crc16_update(0, &data) == u16::from_be_bytes([hi, lo])
    } else {
        let sum = c.byte(T).await?;
        data.iter().fold(0u8, |a, b| a.wrapping_add(*b)) == sum
    };
    if ok && seq ^ cseq == 0xff {
        Ok(Blk::Data(seq, data))
    } else {
        Ok(Blk::Bad)
    }
}

async fn recv_block<S: AsyncRead + AsyncWrite + Unpin + Send>(
    c: &mut Conn<'_, S>,
    mut first: Option<u8>,
    crc: bool,
) -> io::Result<Blk> {
    let size = loop {
        let b = match first.take() {
            Some(b) => b,
            None => match c.byte(T).await {
                Ok(b) => b,
                Err(e) if is_timeout(&e) => return Ok(Blk::Bad),
                Err(e) => return Err(e),
            },
        };
        match b {
            SOH => break 128,
            STX => break 1024,
            EOT => return Ok(Blk::Eot),
            CAN if c.byte(T).await.ok() == Some(CAN) => return Err(aborted()),
            _ => {}
        }
    };
    match read_rest(c, size, crc).await {
        Err(e) if is_timeout(&e) => Ok(Blk::Bad),
        r => r,
    }
}

/// Envía los sondeos de arranque hasta recibir el primer byte del emisor.
async fn start<S: AsyncRead + AsyncWrite + Unpin + Send>(
    c: &mut Conn<'_, S>,
    probes: &[(u8, bool)],
) -> io::Result<(u8, bool)> {
    for &(p, crc) in probes {
        c.write(&[p]).await?;
        match c.byte(PROBE_WAIT).await {
            Ok(b) => return Ok((b, crc)),
            Err(e) if is_timeout(&e) => {}
            Err(e) => return Err(e),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::TimedOut,
        "el emisor no responde",
    ))
}

/// Bloques de datos de un fichero hasta EOT. `limit` (Ymodem) trunca el relleno.
#[allow(clippy::too_many_arguments)]
async fn recv_data<S: AsyncRead + AsyncWrite + Unpin + Send>(
    c: &mut Conn<'_, S>,
    crc: bool,
    mut first: Option<u8>,
    file: &mut tokio::fs::File,
    name: &str,
    limit: Option<u64>,
    progress: &mut (dyn FnMut(Progress) + Send),
) -> io::Result<()> {
    let mut expected = 1u8;
    let mut errors = 0;
    let mut written = 0u64;
    let mut held: Option<Vec<u8>> = None;
    loop {
        match recv_block(c, first.take(), crc).await? {
            Blk::Eot => {
                c.write(&[NAK]).await?;
                let _ = c.byte(T).await; // segundo EOT
                if let Some(mut h) = held.take() {
                    while h.last() == Some(&SUB) {
                        h.pop();
                    }
                    file.write_all(&h).await?;
                }
                file.flush().await?;
                return c.write(&[ACK]).await;
            }
            Blk::Data(seq, d) if seq == expected => {
                errors = 0;
                match limit {
                    Some(lim) => {
                        let take = d
                            .len()
                            .min(usize::try_from(lim - written).unwrap_or(usize::MAX));
                        file.write_all(&d[..take]).await?;
                        written += take as u64;
                    }
                    None => {
                        if let Some(h) = held.replace(d) {
                            file.write_all(&h).await?;
                            written += h.len() as u64;
                        }
                    }
                }
                expected = expected.wrapping_add(1);
                c.write(&[ACK]).await?;
                progress(Progress {
                    file: name.to_string(),
                    done: written + held.as_ref().map_or(0, |h| h.len() as u64),
                    total: limit,
                });
            }
            Blk::Data(seq, _) if seq == expected.wrapping_sub(1) => c.write(&[ACK]).await?,
            Blk::Data(..) => {
                c.cancel().await;
                return Err(proto("número de bloque fuera de secuencia"));
            }
            Blk::Bad => {
                errors += 1;
                if errors > RETRIES {
                    c.cancel().await;
                    return Err(proto("demasiados bloques erróneos"));
                }
                c.write(&[NAK]).await?;
            }
        }
    }
}

pub(crate) async fn receive<S: AsyncRead + AsyncWrite + Unpin + Send>(
    io: &mut S,
    ymodem: bool,
    dest: &Path,
    progress: &mut (dyn FnMut(Progress) + Send),
) -> io::Result<Vec<PathBuf>> {
    let mut c = Conn::new(io);
    let probes: Vec<(u8, bool)> = if ymodem {
        vec![(b'C', true); 10]
    } else {
        let mut v = vec![(b'C', true); 6];
        v.extend([(NAK, false); 6]);
        v
    };
    let (first, crc) = start(&mut c, &probes).await?;
    let mut out = Vec::new();
    if !ymodem {
        let (mut f, path) = create_unique(dest, "xmodem.bin").await?;
        recv_data(
            &mut c,
            crc,
            Some(first),
            &mut f,
            "xmodem.bin",
            None,
            progress,
        )
        .await?;
        out.push(path);
        return Ok(out);
    }
    let mut first = Some(first);
    loop {
        if out.len() >= MAX_FILES {
            c.cancel().await;
            return Err(proto("demasiados ficheros en el lote"));
        }
        let mut errors = 0;
        let hdr = loop {
            match recv_block(&mut c, first.take(), crc).await? {
                Blk::Data(0, d) => break d,
                _ => {
                    errors += 1;
                    if errors > RETRIES {
                        c.cancel().await;
                        return Err(proto("bloque 0 no válido"));
                    }
                    c.write(&[NAK]).await?;
                }
            }
        };
        let end = hdr.iter().position(|&b| b == 0).unwrap_or(hdr.len());
        if end == 0 {
            c.write(&[ACK]).await?;
            return Ok(out);
        }
        let name = sanitize_name(&hdr[..end]);
        // El tamaño anunciado solo trunca el relleno; nunca reserva memoria.
        let digits: Vec<u8> = hdr
            .get(end + 1..)
            .unwrap_or(&[])
            .iter()
            .copied()
            .take_while(u8::is_ascii_digit)
            .collect();
        let size = String::from_utf8(digits)
            .ok()
            .and_then(|s| s.parse::<u64>().ok());
        let (mut f, path) = create_unique(dest, &name).await?;
        out.push(path);
        c.write(&[ACK, b'C']).await?;
        recv_data(&mut c, crc, None, &mut f, &name, size, progress).await?;
        c.write(b"C").await?;
    }
}

async fn send_block<S: AsyncRead + AsyncWrite + Unpin + Send>(
    c: &mut Conn<'_, S>,
    seq: u8,
    data: &[u8],
    size: usize,
    pad: u8,
    crc: bool,
) -> io::Result<()> {
    let mut pkt = Vec::with_capacity(size + 6);
    pkt.extend([if size == 128 { SOH } else { STX }, seq, !seq]);
    pkt.extend_from_slice(data);
    pkt.resize(3 + size, pad);
    if crc {
        let v = crc16_update(0, &pkt[3..]);
        pkt.extend(v.to_be_bytes());
    } else {
        pkt.push(pkt[3..].iter().fold(0u8, |a, b| a.wrapping_add(*b)));
    }
    for _ in 0..RETRIES {
        c.write(&pkt).await?;
        loop {
            match c.byte(T).await {
                Ok(ACK) => return Ok(()),
                Ok(NAK) => break,
                Ok(CAN) => {
                    if c.byte(T).await.ok() == Some(CAN) {
                        return Err(aborted());
                    }
                }
                Ok(_) => {}
                Err(e) if is_timeout(&e) => break,
                Err(e) => return Err(e),
            }
        }
    }
    c.cancel().await;
    Err(io::Error::new(
        io::ErrorKind::TimedOut,
        "demasiados reintentos",
    ))
}

async fn send_eot<S: AsyncRead + AsyncWrite + Unpin + Send>(c: &mut Conn<'_, S>) -> io::Result<()> {
    for _ in 0..RETRIES {
        c.write(&[EOT]).await?;
        match c.byte(T).await {
            Ok(ACK) => return Ok(()),
            Ok(CAN) => return Err(aborted()),
            Ok(_) => {}
            Err(e) if is_timeout(&e) => {}
            Err(e) => return Err(e),
        }
    }
    Err(io::Error::new(io::ErrorKind::TimedOut, "EOT sin confirmar"))
}

async fn wait_c<S: AsyncRead + AsyncWrite + Unpin + Send>(c: &mut Conn<'_, S>) -> io::Result<()> {
    loop {
        match c.byte(START_WAIT).await? {
            b'C' => return Ok(()),
            CAN => return Err(aborted()),
            _ => {}
        }
    }
}

pub(crate) async fn send<S: AsyncRead + AsyncWrite + Unpin + Send>(
    io: &mut S,
    ymodem: bool,
    files: &[PathBuf],
    progress: &mut (dyn FnMut(Progress) + Send),
) -> io::Result<()> {
    let mut c = Conn::new(io);
    let crc = loop {
        match c.byte(START_WAIT).await? {
            b'C' => break true,
            NAK if !ymodem => break false,
            CAN => return Err(aborted()),
            _ => {}
        }
    };
    for (i, path) in files.iter().enumerate() {
        let mut f = tokio::fs::File::open(path).await?;
        let len = f.metadata().await?.len();
        let name = announce_name(path);
        if ymodem {
            if i > 0 {
                wait_c(&mut c).await?;
            }
            let mut h = name.clone().into_bytes();
            h.push(0);
            h.extend(len.to_string().bytes());
            let size = if h.len() <= 128 { 128 } else { 1024 };
            send_block(&mut c, 0, &h, size, 0, crc).await?;
            wait_c(&mut c).await?;
        }
        let bs = if ymodem { 1024 } else { 128 };
        let (mut seq, mut done) = (1u8, 0u64);
        loop {
            let chunk = read_chunk(&mut f, bs).await?;
            if chunk.is_empty() {
                break;
            }
            let size = if chunk.len() <= 128 { 128 } else { 1024 };
            send_block(&mut c, seq, &chunk, size, SUB, crc).await?;
            seq = seq.wrapping_add(1);
            done += chunk.len() as u64;
            progress(Progress {
                file: name.clone(),
                done,
                total: Some(len),
            });
        }
        send_eot(&mut c).await?;
    }
    if ymodem {
        wait_c(&mut c).await?;
        send_block(&mut c, 0, &[], 128, 0, crc).await?;
    }
    c.write(&[]).await
}
