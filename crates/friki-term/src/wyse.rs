//! Traductor de Wyse 50/60 y TeleVideo 910/925 a ANSI/VT, previo a `pre::Pre`.
//!
//! Fuente: entradas `wy50`, `wy60`, `tvi910` y `tvi925` de terminfo (ncurses) y los
//! manuales públicos de los terminales. Máquina de estados que conserva el estado entre
//! llamadas (secuencias partidas entre trozos). Lo desconocido se descarta sin tocar el
//! estado de la pantalla.
//!
//! Sin soporte (se descartan): líneas protegidas y atributos de protección, línea de
//! estado y etiquetas de teclas de función, parámetros de configuración (`ESC c`, `ESC d`,
//! `ESC e`, `ESC w`, `ESC ~` de wy60), impresión, 132 columnas, pantalla en blanco/flash.
//! Los "clear unprotected" (`ESC :`/`ESC ;`) se tratan como borrado total con cursor a
//! home. `ESC c` en wy60 asume un parámetro más salvo `D`/`E` (gráficos).

use friki_core::config::Emulation;

use crate::Key;

#[derive(Clone, Copy, PartialEq)]
enum Op {
    Cup,
    Attr,
    Backtick,
    Gfx,
    Hpa,
    Vpa,
    Dot,
    Cset,
    Ign,
}

#[derive(Clone, Copy, PartialEq)]
enum St {
    Ground,
    Esc,
    /// Recoge `n` bytes tras la secuencia y aplica `Op`.
    Take(Op, u8),
    /// `ESC a RRR R CCC C` de wy60: dígitos hasta `C`.
    Cup60,
    /// Descarta hasta el terminador (CR o DEL), con tope.
    Until(u8, u16),
    /// CSI estándar (`ESC [`) de los terminales que no la usan: se pasa tal cual a `pre`,
    /// que es quien responde DSR/CPR/DA (p. ej. el `ESC [ 6 n` inicial de ConPTY).
    Csi,
    /// `ESC [` de TVI910: VPA de un byte.
    Vpa910,
    /// `ESC [ 5` o `ESC [ 6` en TVI910: si sigue `n` es un DSR, si no, VPA.
    Dsr910(u8),
}

pub(crate) struct Wyse {
    e: Emulation,
    st: St,
    buf: Vec<u8>,
    gfx: bool,
    /// Modo inserción (`vt100` no implementa IRM): cada carácter va precedido de ICH.
    ins: bool,
}

/// (carácter del terminal, equivalente Unicode) en modo gráfico; de `acsc` de terminfo.
const GFX50: &[(u8, char)] = &[
    (b'u', '┘'),
    (b's', '┐'),
    (b'r', '┌'),
    (b'q', '└'),
    (b'x', '┼'),
    (b'z', '─'),
    (b't', '├'),
    (b'y', '┤'),
    (b']', '┴'),
    (b'p', '┬'),
    (b'v', '│'),
];
const GFX60: &[(u8, char)] = &[
    (b'Y', '┘'),
    (b'?', '┐'),
    (b'Z', '┌'),
    (b'@', '└'),
    (b'E', '┼'),
    (b'D', '─'),
    (b'C', '├'),
    (b'4', '┤'),
    (b'A', '┴'),
    (b'B', '┬'),
    (b'3', '│'),
];

const CLEAR: &[u8] = b"\x1b[H\x1b[2J";

pub(crate) fn is_wyse_tvi(e: Emulation) -> bool {
    matches!(
        e,
        Emulation::Wyse50 | Emulation::Wyse60 | Emulation::Tvi910 | Emulation::Tvi925
    )
}

impl Wyse {
    pub(crate) fn new(e: Emulation) -> Self {
        Self {
            e,
            st: St::Ground,
            buf: Vec::new(),
            gfx: false,
            ins: false,
        }
    }

    fn wyse(&self) -> bool {
        matches!(self.e, Emulation::Wyse50 | Emulation::Wyse60)
    }

    fn tvi(&self) -> bool {
        !self.wyse()
    }

    pub(crate) fn feed(&mut self, bytes: &[u8], out: &mut Vec<u8>) {
        for &b in bytes {
            self.byte(b, out);
        }
    }

    fn byte(&mut self, b: u8, out: &mut Vec<u8>) {
        match self.st {
            St::Ground => self.ground(b, out),
            St::Esc => self.esc(b, out),
            St::Take(op, n) => {
                if b == 0x1b {
                    self.buf.clear();
                    self.st = St::Esc;
                    return;
                }
                self.buf.push(b);
                if self.buf.len() >= usize::from(n) {
                    self.st = St::Ground;
                    let buf = std::mem::take(&mut self.buf);
                    self.finish(op, &buf, out);
                }
            }
            St::Cup60 => match b {
                b'0'..=b'9' | b'R' if self.buf.len() < 12 => self.buf.push(b),
                b'C' => {
                    self.st = St::Ground;
                    let buf = std::mem::take(&mut self.buf);
                    let mut it = buf.split(|&c| c == b'R').map(|d| {
                        std::str::from_utf8(d)
                            .ok()
                            .and_then(|s| s.parse::<u32>().ok())
                            .unwrap_or(1)
                            .max(1)
                    });
                    let (r, c) = (it.next().unwrap_or(1), it.next().unwrap_or(1));
                    out.extend_from_slice(format!("\x1b[{r};{c}H").as_bytes());
                }
                _ => {
                    // Secuencia inválida: se descarta y el byte se reprocesa.
                    self.buf.clear();
                    self.st = St::Ground;
                    self.byte(b, out);
                }
            },
            St::Until(t, n) => {
                if b == t {
                    self.st = St::Ground;
                } else if b == 0x1b {
                    self.st = St::Esc;
                } else if n >= 256 {
                    self.st = St::Ground;
                } else {
                    self.st = St::Until(t, n + 1);
                }
            }
            St::Csi => match b {
                0x1b => self.st = St::Esc,
                0x20..=0x3f => out.push(b),
                0x40..=0x7e => {
                    out.push(b);
                    self.st = St::Ground;
                }
                // C0 dentro de CSI: `pre` la ejecuta; el resto se descarta.
                0..=0x1f => out.push(b),
                _ => self.st = St::Ground,
            },
            St::Vpa910 => match b {
                0x1b => self.st = St::Esc,
                b'5' | b'6' => self.st = St::Dsr910(b),
                _ => {
                    self.st = St::Ground;
                    self.finish(Op::Vpa, &[b], out);
                }
            },
            St::Dsr910(q) => {
                self.st = St::Ground;
                if b == b'n' {
                    out.extend_from_slice(format!("\x1b[{}n", char::from(q)).as_bytes());
                } else {
                    self.finish(Op::Vpa, &[q], out);
                    self.byte(b, out);
                }
            }
        }
    }

    fn ground(&mut self, b: u8, out: &mut Vec<u8>) {
        match b {
            0x1b => self.st = St::Esc,
            0x0b => out.extend_from_slice(b"\x1b[A"),
            0x0c => out.extend_from_slice(b"\x1b[C"),
            0x16 if self.e == Emulation::Tvi925 => out.extend_from_slice(b"\x1b[B"),
            0x1e => out.extend_from_slice(b"\x1b[H"),
            0x1a => out.extend_from_slice(CLEAR),
            0x1f => out.extend_from_slice(b"\r\n"),
            // SOH, SO, SI, DC4 (impresión) y ^V fuera de tvi925: sin efecto.
            0x01 | 0x0e | 0x0f | 0x14 | 0x16 => {}
            0x20..=0x7e | 0xc0..=0xff if self.ins => {
                out.extend_from_slice(b"\x1b[@");
                self.ins = false;
                self.ground(b, out);
                self.ins = true;
            }
            0x20..=0x7e if self.gfx => {
                let t = if self.e == Emulation::Wyse60 {
                    GFX60
                } else {
                    GFX50
                };
                match t.iter().find(|(k, _)| *k == b) {
                    Some((_, c)) => {
                        let mut u = [0u8; 4];
                        out.extend_from_slice(c.encode_utf8(&mut u).as_bytes());
                    }
                    None => out.push(b),
                }
            }
            _ => out.push(b),
        }
    }

    fn esc(&mut self, b: u8, out: &mut Vec<u8>) {
        self.st = St::Ground;
        let (wy, tvi, e) = (self.wyse(), self.tvi(), self.e);
        let take = |s: &mut Self, op, n| s.st = St::Take(op, n);
        match b {
            0x1b => self.st = St::Esc,
            b'=' => take(self, Op::Cup, 2),
            b'a' if e == Emulation::Wyse60 => self.st = St::Cup60,
            b'G' => take(self, Op::Attr, 1),
            b'`' if wy => take(self, Op::Backtick, 1),
            b'H' if wy => take(self, Op::Gfx, 1),
            b']' if e == Emulation::Tvi910 => take(self, Op::Hpa, 1),
            b'[' if e == Emulation::Tvi910 => self.st = St::Vpa910,
            b'[' => {
                out.extend_from_slice(b"\x1b[");
                self.st = St::Csi;
            }
            b'.' if tvi => take(self, Op::Dot, 1),
            b'c' if e == Emulation::Wyse60 => take(self, Op::Cset, 1),
            b'd' | b'e' | b'w' | b'~' if e == Emulation::Wyse60 => take(self, Op::Ign, 1),
            b'A' if wy => take(self, Op::Ign, 2),
            b'F' if wy => self.st = St::Until(b'\r', 0),
            b'f' if tvi => self.st = St::Until(b'\r', 0),
            b'z' if wy => self.st = St::Until(b'\r', 0),
            b'Z' if wy => self.st = St::Until(0x7f, 0),
            b'(' if e == Emulation::Wyse50 => out.extend_from_slice(b"\x1b[0m"),
            b'T' | b't' => out.extend_from_slice(b"\x1b[K"),
            b'Y' | b'y' => out.extend_from_slice(b"\x1b[J"),
            b'E' => out.extend_from_slice(b"\x1b[L"),
            b'R' => out.extend_from_slice(b"\x1b[M"),
            b'W' => out.extend_from_slice(b"\x1b[P"),
            b'Q' => out.extend_from_slice(b"\x1b[@"),
            b'q' => self.ins = true,
            b'r' => self.ins = false,
            b'j' => out.extend_from_slice(b"\x1bM"),
            b'I' => out.extend_from_slice(b"\x1b[Z"),
            b'1' => out.extend_from_slice(b"\x1bH"),
            b'2' => out.extend_from_slice(b"\x1b[g"),
            b'0' | b'3' => out.extend_from_slice(b"\x1b[3g"),
            b'*' | b'+' | b':' | b';' => out.extend_from_slice(CLEAR),
            // Controles C0 dentro de ESC: se abandona y se procesa el byte.
            0..=0x1f => self.ground(b, out),
            // Protección (`ESC (`/`ESC )`/`ESC &`...), flash, teclas... y lo desconocido.
            _ => {}
        }
    }

    fn finish(&mut self, op: Op, buf: &[u8], out: &mut Vec<u8>) {
        let n = buf[0];
        match op {
            Op::Cup => {
                let p = |v: u8| u16::from(v.saturating_sub(31)).max(1);
                out.extend_from_slice(format!("\x1b[{};{}H", p(buf[0]), p(buf[1])).as_bytes());
            }
            Op::Hpa => {
                out.extend_from_slice(format!("\x1b[{}G", n.saturating_sub(31).max(1)).as_bytes())
            }
            Op::Vpa => {
                out.extend_from_slice(format!("\x1b[{}d", n.saturating_sub(31).max(1)).as_bytes())
            }
            // Bits: 1 invisible, 2 parpadeo, 4 inverso, 8 subrayado, 0x40 atenuado.
            Op::Attr if n & 0xb0 == 0x30 => {
                let mut s = String::from("\x1b[0");
                for (bit, sgr) in [(1, ";8"), (2, ";5"), (4, ";7"), (8, ";4")] {
                    if n & bit != 0 {
                        s.push_str(sgr);
                    }
                }
                if n & 0x40 != 0 {
                    s.push_str(";2");
                }
                s.push('m');
                out.extend_from_slice(s.as_bytes());
            }
            Op::Backtick => match n {
                b'0' => out.extend_from_slice(b"\x1b[?25l"),
                b'1' => out.extend_from_slice(b"\x1b[?25h"),
                b'6' if self.e == Emulation::Wyse50 => out.extend_from_slice(b"\x1b[7m"),
                b'7' if self.e == Emulation::Wyse50 => out.extend_from_slice(b"\x1b[2m"),
                _ => {}
            },
            Op::Dot => match n {
                b'0' => out.extend_from_slice(b"\x1b[?25l"),
                b'1'..=b'4' => out.extend_from_slice(b"\x1b[?25h"),
                _ => {}
            },
            Op::Gfx => match n {
                2 => self.gfx = true,
                3 => self.gfx = false,
                _ => {}
            },
            Op::Cset => match n {
                b'D' => self.gfx = false,
                b'E' => self.gfx = true,
                _ => self.st = St::Take(Op::Ign, 1),
            },
            Op::Attr | Op::Ign => {}
        }
    }
}

/// Bytes de la tecla según terminfo (`kcu*`, `kf*`, `khome`...). `None`: la tecla se
/// codifica como en el resto de emulaciones (caracteres, Enter, Tab, Esc).
pub(crate) fn encode_key(e: Emulation, key: Key) -> Option<Vec<u8>> {
    let tvi925 = e == Emulation::Tvi925;
    let wy = matches!(e, Emulation::Wyse50 | Emulation::Wyse60);
    let s: &[u8] = match key {
        Key::Char(_) | Key::Enter | Key::Tab | Key::Esc => return None,
        Key::Backspace | Key::Left => b"\x08",
        Key::Up => b"\x0b",
        Key::Down if tvi925 => b"\x16",
        Key::Down => b"\x0a",
        Key::Right => b"\x0c",
        Key::Home => b"\x1e",
        Key::BackTab => b"\x1bI",
        Key::Insert if wy || tvi925 => b"\x1bQ",
        Key::Delete if wy || tvi925 => b"\x1bW",
        Key::PageUp if wy => b"\x1bJ",
        Key::PageDown if wy => b"\x1bK",
        // kf1..kf16 = ^A @..O CR (tvi: hasta kf10 = kf0 `^AI CR`).
        Key::F(n) if (1..=if wy { 16 } else { 10 }).contains(&n) => {
            return Some(vec![1, b'@' + n - 1, b'\r']);
        }
        _ => b"",
    };
    Some(s.to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tr(e: Emulation, chunks: &[&[u8]]) -> Vec<u8> {
        let mut w = Wyse::new(e);
        let mut o = Vec::new();
        for c in chunks {
            w.feed(c, &mut o);
        }
        o
    }
    const W5: Emulation = Emulation::Wyse50;
    const W6: Emulation = Emulation::Wyse60;
    const T9: Emulation = Emulation::Tvi910;
    const T25: Emulation = Emulation::Tvi925;

    #[test]
    fn cup_con_desplazamiento_y_partido() {
        assert_eq!(tr(W5, &[b"\x1b=%0"]), b"\x1b[6;17H");
        assert_eq!(tr(T25, &[b"\x1b", b"=", b" ", b" x"]), b"\x1b[1;1Hx");
        // Por debajo del desplazamiento: fila/columna 1.
        assert_eq!(tr(T9, &[b"\x1b=\x01\x01"]), b"\x1b[1;1H");
    }

    #[test]
    fn cup_wy60_decimal() {
        assert_eq!(tr(W6, &[b"\x1ba12R40C"]), b"\x1b[12;40H");
        assert_eq!(tr(W6, &[b"\x1ba3", b"R", b"7", b"Cz"]), b"\x1b[3;7Hz");
        // Inválida: se descarta y el byte se reprocesa.
        assert_eq!(tr(W6, &[b"\x1ba1x"]), b"x");
    }

    #[test]
    fn borrados_e_insercion() {
        assert_eq!(tr(W5, &[b"\x1bT\x1bt"]), b"\x1b[K\x1b[K");
        assert_eq!(tr(W5, &[b"\x1bY\x1by"]), b"\x1b[J\x1b[J");
        assert_eq!(
            tr(T25, &[b"\x1bE\x1bR\x1bW\x1bQ"]),
            b"\x1b[L\x1b[M\x1b[P\x1b[@"
        );
        assert_eq!(tr(T25, &[b"\x1bqab\x1brc"]), b"\x1b[@a\x1b[@bc");
        assert_eq!(tr(T25, &[b"\x1b+"]), CLEAR);
        assert_eq!(tr(T9, &[b"\x1a"]), CLEAR);
    }

    #[test]
    fn csi_estandar_pasa_tal_cual() {
        // Las consultas (DSR) deben llegar a `pre` para responderlas.
        assert_eq!(tr(W5, &[b"\x1b[6n"]), b"\x1b[6n");
        assert_eq!(tr(T25, &[b"\x1b[", b"6n"]), b"\x1b[6n");
        assert_eq!(tr(W6, &[b"\x1b[6\x1bT"]), b"\x1b[6\x1b[K");
        // TVI910: `ESC [ 5 n` y `ESC [ 6 n` son DSR; el resto sigue siendo VPA.
        assert_eq!(tr(T9, &[b"\x1b[6n"]), b"\x1b[6n");
        assert_eq!(tr(T9, &[b"\x1b[!x"]), b"\x1b[2dx");
        assert_eq!(tr(T9, &[b"\x1b[6!"]), b"\x1b[23d!");
    }

    #[test]
    fn controles_de_cursor() {
        assert_eq!(tr(W5, &[b"\x0b\x0c\x1e\x1f"]), b"\x1b[A\x1b[C\x1b[H\r\n");
        assert_eq!(tr(T25, &[b"\x16"]), b"\x1b[B");
        assert_eq!(tr(T9, &[b"\x16"]), b"");
        assert_eq!(tr(W5, &[b"\x1bj\x1bI"]), b"\x1bM\x1b[Z");
    }

    #[test]
    fn atributos() {
        assert_eq!(tr(T25, &[b"\x1bG4"]), b"\x1b[0;7m");
        assert_eq!(tr(T25, &[b"\x1bG0"]), b"\x1b[0m");
        assert_eq!(tr(W6, &[b"\x1bG8"]), b"\x1b[0;4m");
        assert_eq!(tr(W6, &[b"\x1bGt"]), b"\x1b[0;7;2m");
        assert_eq!(tr(W6, &[b"\x1bG", b"2"]), b"\x1b[0;5m");
        // Parámetro inválido: se descarta.
        assert_eq!(tr(W6, &[b"\x1bGA"]), b"");
    }

    #[test]
    fn wy50_atributos_y_cursor() {
        assert_eq!(tr(W5, &[b"\x1b`6\x1b)ab\x1b("]), b"\x1b[7mab\x1b[0m");
        assert_eq!(tr(W5, &[b"\x1b`0\x1b`1"]), b"\x1b[?25l\x1b[?25h");
        assert_eq!(tr(T25, &[b"\x1b.0\x1b.4"]), b"\x1b[?25l\x1b[?25h");
    }

    #[test]
    fn tabuladores() {
        assert_eq!(tr(T25, &[b"\x1b1\x1b2\x1b3"]), b"\x1bH\x1b[g\x1b[3g");
        assert_eq!(tr(W5, &[b"\x1b0"]), b"\x1b[3g");
    }

    #[test]
    fn hpa_vpa_tvi910() {
        assert_eq!(tr(T9, &[b"\x1b]%\x1b[#"]), b"\x1b[6G\x1b[4d");
    }

    #[test]
    fn protegidos_y_estado_se_descartan() {
        assert_eq!(tr(W6, &[b"a\x1b)b\x1b(c"]), b"abc");
        assert_eq!(tr(W5, &[b"a\x1bFstatus line\rb"]), b"ab");
        assert_eq!(tr(W5, &[b"a\x1bF", b"sta", b"tus\rb"]), b"ab");
        assert_eq!(tr(W5, &[b"a\x1bz1label\rb"]), b"ab");
        assert_eq!(tr(W6, &[b"a\x1bZ1Atext\x7fb"]), b"ab");
        assert_eq!(tr(W5, &[b"a\x1bA10b"]), b"ab");
        assert_eq!(tr(W6, &[b"a\x1bcB0b\x1bd$c"]), b"abc");
        assert_eq!(tr(T25, &[b"a\x1bb\x1bd\x1bhb"]), b"ab");
    }

    #[test]
    fn desconocidas_no_corrompen() {
        assert_eq!(tr(W5, &[b"a\x1b%b\x1b\x01c"]), b"abc");
        // ESC dentro de una secuencia recoge parámetros: reinicia.
        assert_eq!(tr(T25, &[b"\x1b=\x1b=  x"]), b"\x1b[1;1Hx");
    }

    #[test]
    fn graficos() {
        assert_eq!(tr(W5, &[b"\x1bH\x02rzs\x1bH\x03rzs"]), "┌─┐rzs".as_bytes());
        assert_eq!(tr(W6, &[b"\x1bcEZD?\x1bcDZ"]), "┌─┐Z".as_bytes());
    }

    #[test]
    fn teclas() {
        let k = |e, k| encode_key(e, k).unwrap();
        assert_eq!(k(W5, Key::Up), b"\x0b");
        assert_eq!(k(W5, Key::Down), b"\n");
        assert_eq!(k(T25, Key::Down), b"\x16");
        assert_eq!(k(W6, Key::Left), b"\x08");
        assert_eq!(k(W6, Key::Home), b"\x1e");
        assert_eq!(k(W6, Key::Backspace), b"\x08");
        assert_eq!(k(W6, Key::F(1)), b"\x01@\r");
        assert_eq!(k(W6, Key::F(16)), b"\x01O\r");
        assert_eq!(k(T9, Key::F(10)), b"\x01I\r");
        assert_eq!(k(T9, Key::F(11)), b"");
        assert_eq!(k(T9, Key::Delete), b"");
        assert_eq!(k(W5, Key::Delete), b"\x1bW");
        assert_eq!(k(W5, Key::Insert), b"\x1bQ");
        assert_eq!(k(W5, Key::PageUp), b"\x1bJ");
        assert_eq!(k(W5, Key::BackTab), b"\x1bI");
        assert_eq!(encode_key(W5, Key::Char('a')), None);
    }
}
