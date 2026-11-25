//! Preprocesador de bytes delante de `vt100` 0.18.
//!
//! Máquina de estados que conserva su estado entre llamadas (secuencias de escape y
//! secuencias UTF-8 partidas entre `process`). Corrige lo que `vt100` hace distinto de
//! xterm/VT510:
//! - Juegos de caracteres DEC (`ESC ( 0`, `ESC ) 0`, `ESC ( B`, SO/SI): los caracteres
//!   0x5F-0x7E del juego DEC Special Graphics se traducen a Unicode.
//! - `CSI s` / `CSI u` sin parámetros (ANSI.SYS) pasan a `ESC 7` / `ESC 8`.
//! - UTF-8 inválido se sustituye por U+FFFD (una por subsecuencia máxima, como pide
//!   Unicode, cap. 3 "Best Practices for Using U+FFFD").
//! - Paradas de tabulación propias (HTS `ESC H`, TBC `CSI g`): cada HT se traduce a
//!   `CSI n C` hasta la siguiente parada, consultando la columna real del cursor.
//! - DECAWM (`CSI ? 7 l`): con el ajuste desactivado, un carácter con el cursor en espera
//!   de ajuste (columna == ancho) retrocede una columna antes (`CSI D`) y sobrescribe la
//!   última. Límite: un carácter ancho (2 celdas) en la última columna sigue ajustando.

use std::borrow::Cow;

use unicode_width::UnicodeWidthChar;

use crate::TitleCb;

#[derive(Clone, Copy, PartialEq)]
enum St {
    Ground,
    Esc,
    /// `ESC (`, `ESC )`, `ESC *`, `ESC +`: designación del juego G0..G3 (índice).
    EscCs(usize),
    /// Intermedios de otras secuencias ESC (`ESC # 8`, `ESC SP F`...): se pasan tal cual.
    EscInter,
    Csi,
    /// CSI demasiado larga: se pasa tal cual hasta el final.
    CsiRaw,
    /// Cadena OSC (`true`, termina con BEL o ST) o DCS/SOS/PM/APC (`false`, solo ST).
    Str(bool),
}

pub(crate) struct Pre {
    st: St,
    /// G0..G3 designados como DEC Special Graphics.
    graphic: [bool; 4],
    /// Juego activo: 0 = G0 (SI), 1 = G1 (SO).
    shift: usize,
    /// Prefijo de un carácter UTF-8 incompleto del final de la llamada anterior.
    pend: [u8; 4],
    plen: usize,
    csi: Vec<u8>,
    tabs: Vec<bool>,
    /// Las paradas siguen siendo las de fábrica (cada 8): HT pasa tal cual a `vt100`.
    tabs_default: bool,
    awm: bool,
    /// Bytes de la cadena OSC/DCS/APC/PM/SOS en curso y si se está descartando por larga.
    slen: usize,
    sdrop: bool,
    /// Respuestas a consultas del remoto (DSR, CPR, DA) aún sin recoger.
    replies: Vec<u8>,
    /// Respuesta a DA1 según la emulación.
    da1: &'static [u8],
    /// Traductor previo de Wyse/TeleVideo (solo con esas emulaciones).
    wy: Option<crate::wyse::Wyse>,
}

/// Tope de una cadena OSC/DCS/APC/PM; lo que pasa se descarta (CAN a `vt100`).
const MAX_STR: usize = 64 * 1024;

/// DEC Special Graphics (0x5F-0x7E): tabla de xterm (`charsets.c`, `CS_DEC_SPEC_GRAPHIC`)
/// y de la documentación "XTerm Control Sequences" (ctlseqs), apartado "VT100 Mode" /
/// "Character Set". 0x5F es el espacio sin ruptura; 0x6F-0x73 son las líneas de escaneo.
const DEC_GRAPHICS: [char; 32] = [
    '\u{00A0}', '\u{25C6}', '\u{2592}', '\u{2409}', '\u{240C}', '\u{240D}', '\u{240A}', '\u{00B0}',
    '\u{00B1}', '\u{2424}', '\u{240B}', '\u{2518}', '\u{2510}', '\u{250C}', '\u{2514}', '\u{253C}',
    '\u{23BA}', '\u{23BB}', '\u{2500}', '\u{23BC}', '\u{23BD}', '\u{251C}', '\u{2524}', '\u{2534}',
    '\u{252C}', '\u{2502}', '\u{2264}', '\u{2265}', '\u{03C0}', '\u{2260}', '\u{00A3}', '\u{00B7}',
];

/// `vt100` descarta U+FFFD (lo desvía a un callback sin dibujarlo), así que se le pasa este
/// no-carácter Unicode (U+FDD0, nunca válido en texto intercambiado) y las lecturas de
/// celdas lo devuelven a U+FFFD con `unmask`.
const FFFD_MASK: char = '\u{FDD0}';

pub(crate) fn unmask(s: &str) -> Cow<'_, str> {
    if s.contains(FFFD_MASK) {
        Cow::Owned(s.replace(FFFD_MASK, "\u{FFFD}"))
    } else {
        Cow::Borrowed(s)
    }
}

fn default_tabs(from: usize, to: usize) -> impl Iterator<Item = bool> {
    (from..to).map(|c| c % 8 == 0)
}

fn push_char(out: &mut Vec<u8>, c: char) {
    let mut b = [0u8; 4];
    out.extend_from_slice(c.encode_utf8(&mut b).as_bytes());
}

impl Pre {
    pub(crate) fn new(cols: u16, da1: &'static [u8]) -> Self {
        Self {
            st: St::Ground,
            graphic: [false; 4],
            shift: 0,
            pend: [0; 4],
            plen: 0,
            csi: Vec::new(),
            tabs: default_tabs(0, usize::from(cols)).collect(),
            tabs_default: true,
            awm: true,
            slen: 0,
            sdrop: false,
            replies: Vec::new(),
            da1,
            wy: None,
        }
    }

    pub(crate) fn take_replies(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.replies)
    }

    pub(crate) fn emulate(mut self, e: friki_core::config::Emulation) -> Self {
        if crate::wyse::is_wyse_tvi(e) {
            self.wy = Some(crate::wyse::Wyse::new(e));
        }
        self
    }

    pub(crate) fn resize(&mut self, cols: u16) {
        let n = usize::from(cols);
        let old = self.tabs.len();
        self.tabs.truncate(n);
        self.tabs.extend(default_tabs(old, n));
    }

    /// RIS / DECSTR: juegos, ajuste y (solo RIS) paradas de tabulación.
    fn reset(&mut self, hard: bool) {
        self.graphic = [false; 4];
        self.shift = 0;
        self.awm = true;
        if hard {
            let n = self.tabs.len();
            self.tabs = default_tabs(0, n).collect();
            self.tabs_default = true;
        }
    }

    /// Entrega lo pendiente a `vt100` y devuelve (columna del cursor, ancho).
    fn col(out: &mut Vec<u8>, p: &mut vt100::Parser<TitleCb>) -> (u16, u16) {
        p.process(out);
        out.clear();
        let s = p.screen();
        (s.cursor_position().1, s.size().1.get())
    }

    pub(crate) fn run(&mut self, bytes: &[u8], p: &mut vt100::Parser<TitleCb>) {
        if let Some(w) = self.wy.as_mut() {
            let mut t = Vec::with_capacity(bytes.len() + 16);
            w.feed(bytes, &mut t);
            return self.run_vt(&t, p);
        }
        self.run_vt(bytes, p);
    }

    fn run_vt(&mut self, bytes: &[u8], p: &mut vt100::Parser<TitleCb>) {
        let mut out = Vec::with_capacity(bytes.len() + 16);
        let mut i = 0;
        while i < bytes.len() {
            let b = bytes[i];
            match self.st {
                St::Ground if self.plen > 0 => i += self.pending(b, &mut out, p),
                St::Ground if b >= 0x20 && b != 0x7f => {
                    // Fin de la tirada de texto y si es ASCII puro, en una sola pasada.
                    let mut hi = 0;
                    let mut end = i;
                    while let Some(&c) = bytes.get(end) {
                        if c < 0x20 || c == 0x7f {
                            break;
                        }
                        hi |= c;
                        end += 1;
                    }
                    let run = &bytes[i..end];
                    if hi < 0x80 && !self.special() {
                        out.extend_from_slice(run);
                    } else {
                        self.text(run, end == bytes.len(), &mut out, p);
                    }
                    i = end;
                }
                St::Ground => {
                    // CSI completa dentro de la misma entrada: sin pasar por `self.csi`.
                    if b == 0x1b && bytes.get(i + 1) == Some(&b'[') {
                        let body = &bytes[i + 2..];
                        let n = body
                            .iter()
                            .take_while(|&&c| (0x20..=0x3f).contains(&c))
                            .count();
                        if let Some(&f) = body.get(n).filter(|f| (0x40..=0x7e).contains(*f)) {
                            self.csi_final(f, &body[..n], &mut out, p);
                            i += n + 3;
                            continue;
                        }
                    }
                    self.ground_ctrl(b, &mut out, p);
                    i += 1;
                }
                St::Str(osc) => {
                    self.slen += 1;
                    if self.sdrop || self.slen > MAX_STR {
                        if !self.sdrop {
                            self.sdrop = true;
                            out.push(0x18); // CAN: vt100 abandona la cadena
                        }
                    } else {
                        out.push(b);
                    }
                    match b {
                        0x07 if osc => self.st = St::Ground,
                        0x18 | 0x1a => self.st = St::Ground,
                        // `ESC \` (ST) llega por Esc, que lo emite como ESC + final.
                        0x1b => {
                            if !self.sdrop {
                                out.pop();
                            }
                            self.st = St::Esc;
                        }
                        _ => {}
                    }
                    i += 1;
                }
                // Sin avanzar: el byte >= 0x80 se reprocesa en Ground (como UTF-8).
                _ if b >= 0x80 => self.st = St::Ground,
                _ if b < 0x20 || b == 0x7f => {
                    match b {
                        0x18 | 0x1a => self.st = St::Ground,
                        0x1b => {
                            self.csi.clear();
                            self.st = St::Esc;
                        }
                        _ => self.ground_ctrl(b, &mut out, p),
                    }
                    i += 1;
                }
                St::Esc => {
                    self.esc(b, &mut out, p);
                    i += 1;
                }
                St::EscCs(n) => {
                    // ESC ( 0 : DEC Special Graphics; cualquier otro final es otro juego.
                    if (0x30..=0x7e).contains(&b) {
                        self.graphic[n] = b == b'0';
                        out.extend_from_slice(&[0x1b, b"()*+"[n], b]);
                        self.st = St::Ground;
                    } else {
                        // Intermedio adicional (p. ej. `ESC ( %`): se pasa tal cual.
                        out.extend_from_slice(&[0x1b, b"()*+"[n], b]);
                        self.st = St::EscInter;
                    }
                    i += 1;
                }
                St::EscInter => {
                    out.push(b);
                    if b >= 0x30 {
                        self.st = St::Ground;
                    }
                    i += 1;
                }
                St::CsiRaw => {
                    out.push(b);
                    if (0x40..=0x7e).contains(&b) {
                        self.st = St::Ground;
                    }
                    i += 1;
                }
                St::Csi => {
                    if (0x40..=0x7e).contains(&b) {
                        self.st = St::Ground;
                        let params = std::mem::take(&mut self.csi);
                        self.csi_final(b, &params, &mut out, p);
                        // Reutiliza la reserva del buffer.
                        self.csi = params;
                        self.csi.clear();
                    } else if self.csi.len() >= 256 {
                        out.extend_from_slice(b"\x1b[");
                        out.append(&mut self.csi);
                        out.push(b);
                        self.st = St::CsiRaw;
                    } else {
                        self.csi.push(b);
                    }
                    i += 1;
                }
            }
        }
        p.process(&out);
    }

    /// Control C0 (o DEL) fuera de una secuencia. ESC inicia una.
    fn ground_ctrl(&mut self, b: u8, out: &mut Vec<u8>, p: &mut vt100::Parser<TitleCb>) {
        match b {
            0x1b => self.st = St::Esc,
            0x0e => self.shift = 1,
            0x0f => self.shift = 0,
            0x09 if self.tabs_default => out.push(b),
            0x09 => {
                let (c, cols) = Self::col(out, p);
                let last = cols.saturating_sub(1);
                if c < last {
                    let next = (usize::from(c) + 1..usize::from(cols))
                        .find(|&j| self.tabs.get(j).copied().unwrap_or(false))
                        .map_or(last, |j| u16::try_from(j).unwrap_or(last));
                    // `next - c` <= 65535; cifras a mano para no reservar memoria por HT.
                    let mut n = next - c;
                    let mut d = [0u8; 5];
                    let mut k = d.len();
                    while n > 0 {
                        k -= 1;
                        d[k] = b'0' + u8::try_from(n % 10).unwrap_or(0);
                        n /= 10;
                    }
                    out.extend_from_slice(b"\x1b[");
                    out.extend_from_slice(&d[k..]);
                    out.push(b'C');
                }
            }
            _ => out.push(b),
        }
    }

    fn esc(&mut self, b: u8, out: &mut Vec<u8>, p: &mut vt100::Parser<TitleCb>) {
        match b {
            b'[' => {
                self.csi.clear();
                self.st = St::Csi;
            }
            b']' | b'P' | b'X' | b'^' | b'_' => {
                out.extend_from_slice(&[0x1b, b]);
                self.slen = 0;
                self.sdrop = false;
                self.st = St::Str(b == b']');
            }
            b'(' | b')' | b'*' | b'+' => {
                self.st = St::EscCs(usize::from(b.saturating_sub(b'(')));
            }
            b'H' => {
                let (c, _) = Self::col(out, p);
                self.tabs_default = false;
                if let Some(t) = self.tabs.get_mut(usize::from(c)) {
                    *t = true;
                }
                self.st = St::Ground;
            }
            0x20..=0x2f => {
                out.extend_from_slice(&[0x1b, b]);
                self.st = St::EscInter;
            }
            _ => {
                if b == b'c' {
                    self.reset(true);
                }
                out.extend_from_slice(&[0x1b, b]);
                self.st = St::Ground;
            }
        }
    }

    fn csi_final(
        &mut self,
        f: u8,
        params: &[u8],
        out: &mut Vec<u8>,
        p: &mut vt100::Parser<TitleCb>,
    ) {
        match (f, params) {
            // ANSI.SYS: guardar / restaurar cursor. Con parámetros `CSI s` es DECSLRM.
            (b's', []) => out.extend_from_slice(b"\x1b7"),
            (b'u', []) => out.extend_from_slice(b"\x1b8"),
            // Consultas: DSR (estado, posición del cursor) y atributos de dispositivo.
            (b'n', b"5") => self.replies.extend_from_slice(b"\x1b[0n"),
            (b'n', b"6") => {
                let _ = Self::col(out, p);
                let (row, col) = p.screen().cursor_position();
                let cols = p.screen().size().1.get();
                let r = format!("\x1b[{};{}R", row + 1, col.min(cols - 1) + 1);
                self.replies.extend_from_slice(r.as_bytes());
            }
            (b'c', [] | b"0") => self.replies.extend_from_slice(self.da1),
            (b'c', b">" | b">0") => self.replies.extend_from_slice(b"\x1b[>0;10;0c"),
            // CBT (vt100 no lo implementa): a la parada anterior con `CSI n D`.
            (b'Z', [] | b"1") => {
                let (c, _) = Self::col(out, p);
                let c = usize::from(c);
                let prev = (0..c)
                    .rev()
                    .find(|&j| self.tabs.get(j).copied().unwrap_or(false))
                    .unwrap_or(0);
                if c > prev {
                    out.extend_from_slice(format!("\x1b[{}D", c - prev).as_bytes());
                }
            }
            (b'g', [] | b"0") => {
                let (c, _) = Self::col(out, p);
                self.tabs_default = false;
                if let Some(t) = self.tabs.get_mut(usize::from(c)) {
                    *t = false;
                }
            }
            (b'g', b"3") => {
                self.tabs_default = false;
                self.tabs.fill(false);
            }
            _ => {
                if matches!(f, b'h' | b'l') && params.first() == Some(&b'?') {
                    let set = f == b'h';
                    if params[1..].split(|&c| c == b';').any(|n| n == b"7") {
                        self.awm = set;
                    }
                } else if f == b'p' && params == b"!" {
                    self.reset(false);
                }
                out.extend_from_slice(b"\x1b[");
                out.extend_from_slice(params);
                out.push(f);
            }
        }
    }

    /// Un byte más de un carácter UTF-8 incompleto. Devuelve cuántos bytes consume (0 =
    /// el byte no continúa el prefijo y debe reprocesarse).
    fn pending(&mut self, b: u8, out: &mut Vec<u8>, p: &mut vt100::Parser<TitleCb>) -> usize {
        self.pend[self.plen] = b;
        let n = self.plen + 1;
        match std::str::from_utf8(&self.pend[..n]) {
            Ok(s) => {
                let s = s.to_owned();
                self.plen = 0;
                self.chars(&s, out, p);
                1
            }
            Err(e) if e.error_len().is_none() && n < 4 => {
                self.plen = n;
                1
            }
            Err(_) => {
                self.plen = 0;
                self.chars("\u{FFFD}", out, p);
                0
            }
        }
    }

    /// Texto sin controles. `last`: la tirada llega al final de la entrada, así que un
    /// prefijo UTF-8 incompleto puede completarse en la llamada siguiente.
    fn text(&mut self, run: &[u8], last: bool, out: &mut Vec<u8>, p: &mut vt100::Parser<TitleCb>) {
        if let Ok(s) = std::str::from_utf8(run) {
            self.chars(s, out, p);
            return;
        }
        let mut chunks = run.utf8_chunks().peekable();
        while let Some(ch) = chunks.next() {
            self.chars(ch.valid(), out, p);
            let bad = ch.invalid();
            if bad.is_empty() {
                continue;
            }
            let incomplete = std::str::from_utf8(bad)
                .err()
                .is_some_and(|e| e.error_len().is_none());
            if last && incomplete && chunks.peek().is_none() {
                self.pend[..bad.len()].copy_from_slice(bad);
                self.plen = bad.len();
            } else {
                self.chars("\u{FFFD}", out, p);
            }
        }
    }

    fn special(&self) -> bool {
        !self.awm || self.graphic[self.shift]
    }

    fn chars(&mut self, s: &str, out: &mut Vec<u8>, p: &mut vt100::Parser<TitleCb>) {
        if !self.special() && !s.contains('\u{FFFD}') {
            out.extend_from_slice(s.as_bytes());
            return;
        }
        for mut c in s.chars() {
            if c == '\u{FFFD}' {
                c = FFFD_MASK;
            }
            if self.graphic[self.shift] && ('\u{5F}'..='\u{7E}').contains(&c) {
                c = DEC_GRAPHICS[usize::from(c as u8 - 0x5f)];
            }
            if !self.awm && c.width().unwrap_or(0) > 0 {
                let (col, cols) = Self::col(out, p);
                if col >= cols {
                    out.extend_from_slice(b"\x1b[D");
                }
            }
            push_char(out, c);
        }
    }
}
