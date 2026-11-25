//! Intérprete del flujo de datos 3270 saliente (host -> terminal). Nunca entra en pánico:
//! ante datos truncados o direcciones inválidas devuelve error y conserva lo ya aplicado.

use crate::screen::{Cell, Ext, FA_PROTECTED, Screen};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadKind {
    Buffer,
    Modified,
    ModifiedAll,
}

/// Respuesta que el host espera tras procesar un registro.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reply {
    Read(ReadKind),
    Query,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamError {
    Empty,
    Truncated,
    BadAddress(usize),
    UnknownCommand(u8),
    BadStructuredField,
}

/// Cursor de lectura sobre el registro.
struct Rd<'a> {
    d: &'a [u8],
    i: usize,
}

impl<'a> Rd<'a> {
    fn byte(&mut self) -> Result<u8, StreamError> {
        let b = *self.d.get(self.i).ok_or(StreamError::Truncated)?;
        self.i += 1;
        Ok(b)
    }
    fn take(&mut self, n: usize) -> Result<&'a [u8], StreamError> {
        let s = self
            .d
            .get(self.i..self.i.checked_add(n).ok_or(StreamError::Truncated)?)
            .ok_or(StreamError::Truncated)?;
        self.i += n;
        Ok(s)
    }
    fn done(&self) -> bool {
        self.i >= self.d.len()
    }
}

/// Aplica pares tipo/valor de SFE/MF: (atributo de campo si lo hay, extendidos).
fn pairs(p: &[u8], mut ext: Ext) -> (Option<u8>, Ext) {
    let mut fa = None;
    for kv in p.as_chunks::<2>().0 {
        match kv[0] {
            0xC0 => fa = Some(kv[1] & 0x3F),
            0x41 => ext.hl = kv[1],
            0x42 => ext.fg = kv[1],
            0x43 => ext.cs = kv[1],
            _ => {}
        }
    }
    (fa, ext)
}

impl Screen {
    /// Procesa un registro 3270 completo (comando + datos). Devuelve las respuestas debidas.
    pub fn process(&mut self, data: &[u8]) -> Result<Vec<Reply>, StreamError> {
        let mut replies = Vec::new();
        self.command(data, &mut replies, 0)?;
        Ok(replies)
    }

    fn command(&mut self, data: &[u8], out: &mut Vec<Reply>, depth: u8) -> Result<(), StreamError> {
        let (&cmd, rest) = data.split_first().ok_or(StreamError::Empty)?;
        match cmd {
            0xF1 | 0x01 => self.write(rest),
            0xF5 | 0x05 => {
                self.erase(false);
                self.write(rest)
            }
            0x7E | 0x0D => {
                self.erase(true);
                self.write(rest)
            }
            0xF3 | 0x11 if depth == 0 => self.wsf(rest, out),
            0xF2 | 0x02 => {
                out.push(Reply::Read(ReadKind::Buffer));
                Ok(())
            }
            0xF6 | 0x06 => {
                out.push(Reply::Read(ReadKind::Modified));
                Ok(())
            }
            0x6E | 0x0E => {
                out.push(Reply::Read(ReadKind::ModifiedAll));
                Ok(())
            }
            0x6F | 0x0F => {
                self.erase_unprotected();
                self.home();
                self.set_locked(false);
                Ok(())
            }
            c => Err(StreamError::UnknownCommand(c)),
        }
    }

    fn wsf(&mut self, rest: &[u8], out: &mut Vec<Reply>) -> Result<(), StreamError> {
        let mut i = 0;
        while i < rest.len() {
            let hdr = rest.get(i..i + 3).ok_or(StreamError::Truncated)?;
            let len = usize::from(u16::from_be_bytes([hdr[0], hdr[1]]));
            if len < 3 {
                return Err(StreamError::BadStructuredField);
            }
            let body = rest.get(i + 3..i + len).ok_or(StreamError::Truncated)?;
            i += len;
            match (hdr[2], body) {
                // Read Partition: Query / Query List
                (0x01, [_, 0x02 | 0x03, ..]) => out.push(Reply::Query),
                (0x01, [_, 0xF2, ..]) => out.push(Reply::Read(ReadKind::Buffer)),
                (0x01, [_, 0xF6, ..]) => out.push(Reply::Read(ReadKind::Modified)),
                (0x01, [_, 0x6E, ..]) => out.push(Reply::Read(ReadKind::ModifiedAll)),
                // Erase/Reset: bit 0x80 = tamaño alterno
                (0x03, [f, ..]) => self.erase(f & 0x80 != 0),
                // Outbound 3270DS: partición + comando 3270
                (0x40, [_, cmd @ ..]) => self.command(cmd, out, 1)?,
                _ => {}
            }
        }
        Ok(())
    }

    fn addr(&self, rd: &mut Rd) -> Result<usize, StreamError> {
        let b = rd.take(2)?;
        let a = Self::decode_addr(b[0], b[1]);
        if a >= self.cells.len() {
            return Err(StreamError::BadAddress(a));
        }
        Ok(a)
    }

    fn put(&mut self, addr: &mut usize, ch: u8, ext: Ext) {
        self.cells[*addr] = Cell { ch, fa: None, ext };
        *addr = (*addr + 1) % self.cells.len();
    }

    fn put_attr(&mut self, addr: &mut usize, fa: u8, ext: Ext) {
        self.cells[*addr] = Cell {
            ch: 0,
            fa: Some(fa),
            ext,
        };
        *addr = (*addr + 1) % self.cells.len();
    }

    /// WCC + órdenes y datos.
    fn write(&mut self, d: &[u8]) -> Result<(), StreamError> {
        let n = self.cells.len();
        let mut rd = Rd { d, i: 0 };
        let wcc = rd.byte()?;
        if wcc & 0x01 != 0 {
            self.clear_mdt();
        }
        self.sa = Ext::default();
        let mut addr = self.cursor;
        let res = self.orders(&mut rd, &mut addr, n);
        // aun con error se aplican alarma y desbloqueo: el teclado no debe quedar colgado
        if wcc & 0x04 != 0 {
            self.set_alarm();
        }
        if wcc & 0x02 != 0 {
            self.set_locked(false);
        }
        res
    }

    fn orders(&mut self, rd: &mut Rd, addr: &mut usize, n: usize) -> Result<(), StreamError> {
        while !rd.done() {
            match rd.byte()? {
                0x1D => {
                    let a = rd.byte()? & 0x3F;
                    self.put_attr(addr, a, Ext::default());
                }
                0x29 => {
                    let c = usize::from(rd.byte()?);
                    let (fa, ext) = pairs(rd.take(c * 2)?, Ext::default());
                    self.put_attr(addr, fa.unwrap_or(0), ext);
                }
                0x11 => *addr = self.addr(rd)?,
                0x28 => {
                    let kv = rd.take(2)?;
                    match kv[0] {
                        0x00 => self.sa = Ext::default(),
                        0x41 => self.sa.hl = kv[1],
                        0x42 => self.sa.fg = kv[1],
                        0x43 => self.sa.cs = kv[1],
                        _ => {}
                    }
                }
                0x2C => {
                    let c = usize::from(rd.byte()?);
                    let (fa, ext) = pairs(rd.take(c * 2)?, self.cells[*addr].ext);
                    if let Some(f) = self.cells[*addr].fa.as_mut() {
                        if let Some(v) = fa {
                            *f = v;
                        }
                        self.cells[*addr].ext = ext;
                    }
                }
                0x13 => self.cursor = *addr,
                0x05 => self.program_tab(addr),
                0x3C => {
                    let end = self.addr(rd)?;
                    let mut ch = rd.byte()?;
                    let mut ext = self.sa;
                    if ch == 0x08 {
                        ch = rd.byte()?;
                        ext.cs = 0xF1;
                    }
                    for k in 0..n {
                        if k > 0 && *addr == end {
                            break;
                        }
                        self.put(addr, ch, ext);
                    }
                }
                0x12 => {
                    let end = self.addr(rd)?;
                    for k in 0..n {
                        if k > 0 && *addr == end {
                            break;
                        }
                        let p = *addr;
                        if self.cells[p].fa.is_none() && !self.is_protected(p) {
                            self.cells[p] = Cell::default();
                        }
                        *addr = (p + 1) % n;
                    }
                }
                0x08 => {
                    let ch = rd.byte()?;
                    let ext = Ext {
                        cs: 0xF1,
                        ..self.sa
                    };
                    self.put(addr, ch, ext);
                }
                b => {
                    let ext = self.sa;
                    self.put(addr, b, ext);
                }
            }
        }
        Ok(())
    }

    /// PT simplificado: salta al inicio del siguiente campo no protegido; si el campo actual
    /// es de entrada, rellena de nulos lo saltado.
    fn program_tab(&mut self, addr: &mut usize) {
        let n = self.cells.len();
        let clear = !self.is_protected(*addr) && self.has_fields();
        let mut p = *addr;
        for _ in 0..n {
            if self.cells[p].fa.is_some_and(|f| f & FA_PROTECTED == 0) {
                *addr = (p + 1) % n;
                return;
            }
            if clear && self.cells[p].fa.is_none() {
                self.cells[p] = Cell::default();
            }
            p = (p + 1) % n;
        }
        *addr = 0;
    }
}
