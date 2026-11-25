//! Teclas del terminal 3270 y su decodificación desde los bytes que el panel envía
//! (secuencias xterm/vt220/linux de `friki-term`).

use crate::screen::aid;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Char(char),
    Aid(u8),
    Tab,
    BackTab,
    Backspace,
    Delete,
    EraseEof,
    EraseInput,
    Up,
    Down,
    Left,
    Right,
    Home,
    Reset,
}

fn f_key(n: u8, shift: bool) -> Option<Key> {
    aid::pf(if shift { n + 12 } else { n }).map(Key::Aid)
}

/// Mapa: Enter=Enter, F1-F12=PF1-12, Shift+F1-F12=PF13-24, RePág/AvPág=PF7/PF8,
/// Esc=Clear, Ctrl+C=PA1, Ctrl+B=PA2, Ctrl+N=PA3, Ctrl+R=Reset, Ctrl+E=Erase EOF,
/// Ctrl+U=Erase Input, Tab/Shift+Tab entre campos.
pub fn decode(b: &[u8]) -> Vec<Key> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            0x1B => {
                if i + 1 == b.len() {
                    out.push(Key::Aid(aid::CLEAR));
                    i += 1;
                } else {
                    let (k, used) = escape(&b[i + 1..]);
                    out.extend(k);
                    i += 1 + used;
                }
            }
            0x0D | 0x0A => {
                out.push(Key::Aid(aid::ENTER));
                i += 1;
            }
            0x09 => {
                out.push(Key::Tab);
                i += 1;
            }
            0x08 | 0x7F => {
                out.push(Key::Backspace);
                i += 1;
            }
            0x03 => {
                out.push(Key::Aid(aid::PA1));
                i += 1;
            }
            0x02 => {
                out.push(Key::Aid(aid::PA2));
                i += 1;
            }
            0x0E => {
                out.push(Key::Aid(aid::PA3));
                i += 1;
            }
            0x12 => {
                out.push(Key::Reset);
                i += 1;
            }
            0x05 => {
                out.push(Key::EraseEof);
                i += 1;
            }
            0x15 => {
                out.push(Key::EraseInput);
                i += 1;
            }
            0x00..=0x1F => i += 1,
            _ => {
                let end = b[i..]
                    .iter()
                    .position(|&c| c < 0x20 || c == 0x7F)
                    .map_or(b.len(), |p| i + p);
                out.extend(
                    String::from_utf8_lossy(&b[i..end])
                        .chars()
                        .filter(|&c| c != '\u{FFFD}')
                        .map(Key::Char),
                );
                i = end;
            }
        }
    }
    out
}

/// Interpreta lo que sigue a ESC; devuelve la tecla (si la hay) y los bytes consumidos.
fn escape(b: &[u8]) -> (Option<Key>, usize) {
    match b[0] {
        b'[' => {
            if b.get(1) == Some(&b'[') {
                // linux: ESC [ [ A..E = F1..F5
                let k = b.get(2).and_then(|c| match c {
                    b'A'..=b'E' => f_key(c - b'A' + 1, false),
                    _ => None,
                });
                return (k, 3.min(b.len()));
            }
            let end = b[1..]
                .iter()
                .position(|c| (0x40..=0x7E).contains(c))
                .map(|p| p + 1);
            let Some(end) = end else {
                return (None, b.len());
            };
            let params = String::from_utf8_lossy(&b[1..end]);
            let mut it = params.split(';');
            let n: u8 = it.next().and_then(|s| s.parse().ok()).unwrap_or(0);
            let shift = it.next().is_some_and(|m| m == "2");
            let k = match b[end] {
                b'A' => Some(Key::Up),
                b'B' => Some(Key::Down),
                b'C' => Some(Key::Right),
                b'D' => Some(Key::Left),
                b'H' => Some(Key::Home),
                b'Z' => Some(Key::BackTab),
                c @ b'P'..=b'S' => f_key(c - b'P' + 1, shift),
                b'~' => match n {
                    1 => Some(Key::Home),
                    3 => Some(Key::Delete),
                    5 => aid::pf(7).map(Key::Aid),
                    6 => aid::pf(8).map(Key::Aid),
                    11..=15 => f_key(n - 10, shift),
                    17..=21 => f_key(n - 11, shift),
                    23 | 24 => f_key(n - 12, shift),
                    _ => None,
                },
                _ => None,
            };
            (k, end + 1)
        }
        b'O' => {
            let mut j = 1;
            while b.get(j).is_some_and(|c| c.is_ascii_digit() || *c == b';') {
                j += 1;
            }
            let shift = b[1..j.min(b.len())].ends_with(b"2");
            let k = b.get(j).and_then(|c| match c {
                b'A' => Some(Key::Up),
                b'B' => Some(Key::Down),
                b'C' => Some(Key::Right),
                b'D' => Some(Key::Left),
                b'H' => Some(Key::Home),
                b'P'..=b'S' => f_key(c - b'P' + 1, shift),
                _ => None,
            });
            (k, (j + 1).min(b.len()))
        }
        _ => (None, 1),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mapa_de_teclas() {
        assert_eq!(decode(b"\r"), [Key::Aid(aid::ENTER)]);
        assert_eq!(decode(b"\x1b"), [Key::Aid(aid::CLEAR)]);
        assert_eq!(decode(b"\x03"), [Key::Aid(aid::PA1)]);
        assert_eq!(decode(b"\x1bOP"), [Key::Aid(0xF1)]);
        assert_eq!(decode(b"\x1b[15~"), [Key::Aid(0xF5)]);
        assert_eq!(decode(b"\x1b[24~"), [Key::Aid(0x7C)]);
        assert_eq!(decode(b"\x1b[1;2P"), [Key::Aid(0xC1)]);
        assert_eq!(decode(b"\x1b[24;2~"), [Key::Aid(0x4C)]);
        assert_eq!(decode(b"\x1b[Z\t"), [Key::BackTab, Key::Tab]);
        assert_eq!(decode("añ".as_bytes()), [Key::Char('a'), Key::Char('ñ')]);
        // basura truncada: sin pánico
        assert!(decode(b"\x1b[").is_empty());
        assert!(decode(b"\x1b[[").is_empty());
        assert!(decode(b"\x1bO").is_empty());
    }
}
