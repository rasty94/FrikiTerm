//! Búfer de pantalla 3270: celdas, campos, cursor y entrada de teclado (lado cliente).

use crate::ebcdic;

/// Códigos AID (IBM 3270 Data Stream Programmer's Reference).
pub mod aid {
    pub const NONE: u8 = 0x60;
    pub const ENTER: u8 = 0x7D;
    pub const CLEAR: u8 = 0x6D;
    pub const PA1: u8 = 0x6C;
    pub const PA2: u8 = 0x6E;
    pub const PA3: u8 = 0x6B;
    pub const SYSREQ: u8 = 0xF0;

    /// PF1..PF24.
    pub fn pf(n: u8) -> Option<u8> {
        const PF: [u8; 24] = [
            0xF1, 0xF2, 0xF3, 0xF4, 0xF5, 0xF6, 0xF7, 0xF8, 0xF9, 0x7A, 0x7B, 0x7C, 0xC1, 0xC2,
            0xC3, 0xC4, 0xC5, 0xC6, 0xC7, 0xC8, 0xC9, 0x4A, 0x4B, 0x4C,
        ];
        PF.get(usize::from(n).checked_sub(1)?).copied()
    }

    /// AID de lectura corta: solo viaja el AID, sin cursor ni campos.
    pub fn is_short(a: u8) -> bool {
        matches!(a, CLEAR | PA1 | PA2 | PA3 | SYSREQ)
    }
}

pub const FA_PROTECTED: u8 = 0x20;
pub const FA_NUMERIC: u8 = 0x10;
pub const FA_DISPLAY: u8 = 0x0C;
pub const FA_MDT: u8 = 0x01;
/// Valor de `FA_DISPLAY` para campos no visibles.
pub const DISP_HIDDEN: u8 = 0x0C;
pub const DISP_INTENSE: u8 = 0x08;

/// Código de 6 bits <-> byte de la trama (direcciones de 12 bits y atributos).
const CODE6: [u8; 64] = [
    0x40, 0xC1, 0xC2, 0xC3, 0xC4, 0xC5, 0xC6, 0xC7, 0xC8, 0xC9, 0x4A, 0x4B, 0x4C, 0x4D, 0x4E, 0x4F,
    0x50, 0xD1, 0xD2, 0xD3, 0xD4, 0xD5, 0xD6, 0xD7, 0xD8, 0xD9, 0x5A, 0x5B, 0x5C, 0x5D, 0x5E, 0x5F,
    0x60, 0x61, 0xE2, 0xE3, 0xE4, 0xE5, 0xE6, 0xE7, 0xE8, 0xE9, 0x6A, 0x6B, 0x6C, 0x6D, 0x6E, 0x6F,
    0xF0, 0xF1, 0xF2, 0xF3, 0xF4, 0xF5, 0xF6, 0xF7, 0xF8, 0xF9, 0x7A, 0x7B, 0x7C, 0x7D, 0x7E, 0x7F,
];

pub(crate) fn code6(v: u8) -> u8 {
    CODE6[usize::from(v & 0x3F)]
}

/// Atributos extendidos de carácter (SA / SFE / MF): resaltado, color, juego.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Ext {
    /// 0 = por defecto, 0xF1 parpadeo, 0xF2 inverso, 0xF4 subrayado.
    pub hl: u8,
    /// 0 = por defecto, 0xF1..0xF7 azul..blanco.
    pub fg: u8,
    /// 0 = base, 0xF1 = escape gráfico (GE).
    pub cs: u8,
}

/// Celda: `fa = Some(_)` es la posición de un atributo de campo (ocupa una celda).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Cell {
    /// Carácter en EBCDIC (0 = nulo).
    pub ch: u8,
    /// Atributo de campo (6 bits bajos).
    pub fa: Option<u8>,
    pub ext: Ext,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputError {
    /// Teclado bloqueado a la espera del host.
    Locked,
    /// Cursor en un campo protegido o sobre un atributo.
    Protected,
    /// Carácter no permitido en un campo numérico.
    Numeric,
    /// Carácter sin código EBCDIC.
    Unmappable,
}

#[derive(Debug, Clone)]
pub struct Screen {
    rows: usize,
    cols: usize,
    pub(crate) cells: Vec<Cell>,
    pub(crate) cursor: usize,
    locked: bool,
    alarm: bool,
    default_size: (usize, usize),
    alt_size: (usize, usize),
    pub(crate) sa: Ext,
    pub(crate) last_aid: u8,
}

impl Screen {
    /// Modelo 2 (24x80), 3 (32x80), 4 (43x80) o 5 (27x132); otro valor = 2.
    pub fn model(m: u8) -> Self {
        let alt = match m {
            3 => (32, 80),
            4 => (43, 80),
            5 => (27, 132),
            _ => (24, 80),
        };
        Self::new((24, 80), alt)
    }

    pub fn new(default_size: (usize, usize), alt_size: (usize, usize)) -> Self {
        let (rows, cols) = default_size;
        Self {
            rows,
            cols,
            cells: vec![Cell::default(); rows * cols],
            cursor: 0,
            locked: true,
            alarm: false,
            default_size,
            alt_size,
            sa: Ext::default(),
            last_aid: aid::NONE,
        }
    }

    pub fn rows(&self) -> usize {
        self.rows
    }
    pub fn cols(&self) -> usize {
        self.cols
    }
    pub fn default_size(&self) -> (usize, usize) {
        self.default_size
    }
    pub fn alt_size(&self) -> (usize, usize) {
        self.alt_size
    }
    pub fn cursor(&self) -> usize {
        self.cursor
    }
    /// (fila, columna) del cursor, base 0.
    pub fn cursor_pos(&self) -> (usize, usize) {
        (self.cursor / self.cols, self.cursor % self.cols)
    }
    pub fn keyboard_locked(&self) -> bool {
        self.locked
    }
    pub(crate) fn set_locked(&mut self, v: bool) {
        self.locked = v;
    }
    /// Devuelve y limpia la petición de alarma del último Write.
    pub fn take_alarm(&mut self) -> bool {
        std::mem::take(&mut self.alarm)
    }
    pub(crate) fn set_alarm(&mut self) {
        self.alarm = true;
    }
    pub fn cell(&self, row: usize, col: usize) -> Option<&Cell> {
        if col >= self.cols {
            return None;
        }
        self.cells.get(row * self.cols + col)
    }
    pub fn has_fields(&self) -> bool {
        self.cells.iter().any(|c| c.fa.is_some())
    }

    /// Reinicia el búfer con otro tamaño (Erase/Write usa el por defecto, EWA el alterno).
    pub(crate) fn erase(&mut self, alt: bool) {
        let (r, c) = if alt {
            self.alt_size
        } else {
            self.default_size
        };
        self.rows = r;
        self.cols = c;
        self.cells = vec![Cell::default(); r * c];
        self.cursor = 0;
        self.sa = Ext::default();
    }

    /// Atributo del campo que contiene `pos` (incluida la propia posición si es atributo).
    pub fn field_at(&self, pos: usize) -> Option<(usize, u8)> {
        let n = self.cells.len();
        (0..n).find_map(|k| {
            let p = (pos + n - k) % n;
            self.cells[p].fa.map(|f| (p, f))
        })
    }

    /// Texto legible de la celda y su atributo vigente, para quien renderiza.
    pub fn is_protected(&self, pos: usize) -> bool {
        self.writable(pos).is_err()
    }

    fn writable(&self, pos: usize) -> Result<u8, InputError> {
        if self.cells[pos].fa.is_some() {
            return Err(InputError::Protected);
        }
        match self.field_at(pos) {
            None => Ok(0),
            Some((_, fa)) if fa & FA_PROTECTED != 0 => Err(InputError::Protected),
            Some((_, fa)) => Ok(fa),
        }
    }

    fn set_mdt(&mut self, pos: usize) {
        if let Some((p, _)) = self.field_at(pos)
            && let Some(f) = self.cells[p].fa.as_mut()
        {
            *f |= FA_MDT;
        }
    }

    pub(crate) fn clear_mdt(&mut self) {
        for c in &mut self.cells {
            if let Some(f) = c.fa.as_mut() {
                *f &= !FA_MDT;
            }
        }
    }

    /// Primera posición de datos de cada campo no protegido, ordenadas.
    fn input_starts(&self) -> Vec<usize> {
        let n = self.cells.len();
        let mut v: Vec<usize> = self
            .cells
            .iter()
            .enumerate()
            .filter(|(_, c)| c.fa.is_some_and(|f| f & FA_PROTECTED == 0))
            .map(|(i, _)| (i + 1) % n)
            .collect();
        v.sort_unstable();
        v
    }

    // ---- entrada del usuario ----

    /// Teclea un carácter en el cursor respetando campos protegidos y numéricos.
    pub fn type_char(&mut self, c: char) -> Result<(), InputError> {
        if self.locked {
            return Err(InputError::Locked);
        }
        let b = ebcdic::from_char(c).ok_or(InputError::Unmappable)?;
        let pos = self.cursor;
        let fa = self.writable(pos)?;
        if fa & FA_NUMERIC != 0 && !(c.is_ascii_digit() || matches!(c, '.' | '-' | ',' | '+')) {
            return Err(InputError::Numeric);
        }
        self.cells[pos] = Cell {
            ch: b,
            fa: None,
            ext: Ext::default(),
        };
        self.set_mdt(pos);
        let next = (pos + 1) % self.cells.len();
        self.cursor = next;
        if self.cells[next].fa.is_some() {
            // fin de campo: salto automático al siguiente campo de entrada
            self.tab();
        }
        Ok(())
    }

    /// Tabulador: inicio del siguiente campo no protegido (con vuelta); 0 si no hay.
    pub fn tab(&mut self) {
        let s = self.input_starts();
        self.cursor = s
            .iter()
            .copied()
            .find(|&p| p > self.cursor)
            .or_else(|| s.first().copied())
            .unwrap_or(0);
    }

    /// Tabulador inverso: inicio del campo actual o, si ya está en él, del anterior.
    pub fn backtab(&mut self) {
        let s = self.input_starts();
        self.cursor = s
            .iter()
            .rev()
            .copied()
            .find(|&p| p < self.cursor)
            .or_else(|| s.last().copied())
            .unwrap_or(0);
    }

    pub fn home(&mut self) {
        self.cursor = self.input_starts().first().copied().unwrap_or(0);
    }

    pub fn move_cursor(&mut self, drow: isize, dcol: isize) {
        let (r, c) = (self.rows as isize, self.cols as isize);
        let (cr, cc) = self.cursor_pos();
        let nr = (cr as isize + drow).rem_euclid(r);
        let nc = (cc as isize + dcol).rem_euclid(c);
        self.cursor = (nr * c + nc) as usize;
    }

    /// Retroceso: mueve a la izquierda y borra si es un dato editable.
    pub fn backspace(&mut self) -> Result<(), InputError> {
        if self.locked {
            return Err(InputError::Locked);
        }
        let n = self.cells.len();
        let p = (self.cursor + n - 1) % n;
        self.writable(p)?;
        self.cursor = p;
        self.cells[p] = Cell::default();
        self.set_mdt(p);
        Ok(())
    }

    /// Suprime el carácter bajo el cursor desplazando el resto del campo.
    pub fn delete(&mut self) -> Result<(), InputError> {
        if self.locked {
            return Err(InputError::Locked);
        }
        let start = self.cursor;
        self.writable(start)?;
        let n = self.cells.len();
        let mut p = start;
        loop {
            let next = (p + 1) % n;
            if self.cells[next].fa.is_some() || next == start {
                break;
            }
            self.cells[p] = self.cells[next];
            p = next;
        }
        self.cells[p] = Cell::default();
        self.set_mdt(start);
        Ok(())
    }

    /// Borra desde el cursor hasta el final del campo.
    pub fn erase_eof(&mut self) -> Result<(), InputError> {
        if self.locked {
            return Err(InputError::Locked);
        }
        let start = self.cursor;
        self.writable(start)?;
        let n = self.cells.len();
        let mut p = start;
        while self.cells[p].fa.is_none() {
            self.cells[p] = Cell::default();
            p = (p + 1) % n;
            if p == start {
                break;
            }
        }
        self.set_mdt(start);
        Ok(())
    }

    /// Borra todos los campos no protegidos y reinicia MDT (tecla Erase Input).
    pub fn erase_input(&mut self) {
        if self.locked {
            return;
        }
        self.erase_unprotected();
        self.home();
    }

    pub(crate) fn erase_unprotected(&mut self) {
        let unformatted = !self.has_fields();
        for p in 0..self.cells.len() {
            if self.cells[p].fa.is_none() && (unformatted || self.writable(p).is_ok()) {
                self.cells[p] = Cell::default();
            }
        }
        self.clear_mdt();
    }

    /// Tecla Reset: desbloquea el teclado.
    pub fn reset(&mut self) {
        self.locked = false;
    }

    // ---- entrada hacia el host ----

    fn encode_addr(&self, a: usize) -> [u8; 2] {
        if self.cells.len() <= 4096 {
            [code6((a >> 6) as u8), code6(a as u8)]
        } else {
            [((a >> 8) as u8) & 0x3F, a as u8]
        }
    }

    pub(crate) fn decode_addr(b1: u8, b2: u8) -> usize {
        if b1 & 0xC0 == 0 {
            (usize::from(b1 & 0x3F) << 8) | usize::from(b2)
        } else {
            (usize::from(b1 & 0x3F) << 6) | usize::from(b2 & 0x3F)
        }
    }

    /// Pulsación de una tecla AID: devuelve el registro entrante (sin IAC EOR) y bloquea
    /// el teclado. Clear y PA solo envían el AID; el host decide qué redibujar.
    pub fn press_aid(&mut self, a: u8) -> Vec<u8> {
        self.last_aid = a;
        self.locked = true;
        if aid::is_short(a) {
            return vec![a];
        }
        self.read_modified(a, false)
    }

    /// AID + cursor + campos modificados (`SBA addr datos`). `all` incluye todos los campos
    /// (Read Modified All). Sin campos: todo el búfer no nulo.
    pub fn read_modified(&self, a: u8, all: bool) -> Vec<u8> {
        let n = self.cells.len();
        let mut out = vec![a];
        out.extend(self.encode_addr(self.cursor));
        if !self.has_fields() {
            out.extend(self.cells.iter().map(|c| c.ch).filter(|&b| b != 0));
            return out;
        }
        for i in 0..n {
            if !self.cells[i].fa.is_some_and(|f| all || f & FA_MDT != 0) {
                continue;
            }
            let start = (i + 1) % n;
            out.push(0x11);
            out.extend(self.encode_addr(start));
            let mut p = start;
            for _ in 0..n {
                let c = &self.cells[p];
                if c.fa.is_some() {
                    break;
                }
                if c.ch != 0 {
                    out.push(c.ch);
                }
                p = (p + 1) % n;
            }
        }
        out
    }

    /// Respuesta a Read Buffer: AID + cursor + todo el búfer (atributos como `SF`).
    pub fn read_buffer(&self, a: u8) -> Vec<u8> {
        let mut out = vec![a];
        out.extend(self.encode_addr(self.cursor));
        for c in &self.cells {
            match c.fa {
                Some(f) => out.extend([0x1D, code6(f)]),
                None => out.push(c.ch),
            }
        }
        out
    }

    /// Respuesta al Read Partition Query: resumen, área utilizable y partición implícita.
    pub fn query_reply(&self) -> Vec<u8> {
        fn sf(q: u8, body: &[u8]) -> Vec<u8> {
            let len = (body.len() + 4) as u16;
            let mut v = len.to_be_bytes().to_vec();
            v.extend([0x81, q]);
            v.extend_from_slice(body);
            v
        }
        let w = |n: usize| (n as u16).to_be_bytes();
        let (dr, dc) = self.default_size;
        let (ar, ac) = self.alt_size;
        let mut usable = vec![0x01, 0x00];
        usable.extend(w(ac));
        usable.extend(w(ar));
        usable.extend([0x01, 0, 1, 0, 1, 0, 1, 0, 1, 9, 16]);
        usable.extend(w(ac * ar));
        let mut implicit = vec![0x00, 0x00, 0x0B, 0x01, 0x00];
        implicit.extend(w(dc));
        implicit.extend(w(dr));
        implicit.extend(w(ac));
        implicit.extend(w(ar));
        let mut out = vec![0x88];
        out.extend(sf(0x80, &[0x80, 0x81, 0xA6]));
        out.extend(sf(0x81, &usable));
        out.extend(sf(0xA6, &implicit));
        out
    }
}
