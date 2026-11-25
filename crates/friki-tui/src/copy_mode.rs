//! Modo copia estilo tmux/vi sobre el buffer de un panel (scrollback + pantalla).
//!
//! Uso desde el bucle de la app:
//! - Al entrar: `self.copy = Some(CopyMode::new(&term))` (el cursor empieza en el
//!   cursor del remoto).
//! - Mientras sea `Some`, TODAS las teclas del panel van a
//!   `copy.handle_key(&key, &mut term)` y no al remoto. Según el `CopyOutcome`:
//!   `Continue` = nada; `Cancel` = poner `copy = None`; `Copy(texto)` = poner
//!   `copy = None`, llamar a `copy_to_clipboard(&texto)` y, si devuelve
//!   `Clipboard::Osc52(seq)`, escribir `seq` tal cual a la terminal anfitriona
//!   (stdout) para que la copia llegue a la máquina local por SSH/tmux; si da
//!   `Err(msg)`, mostrar `msg` en la barra de estado. Al salir conviene
//!   `term.scroll_view(0)` para volver al final.
//! - En cada dibujo, tras pintar el `Screen`: `copy.render(frame, area, &term)`.
//!
//! Teclas: h/j/k/l y flechas, w/b, 0/$, g/G, PgUp/PgDn; `v` selección normal,
//! Ctrl+v rectangular (repetir la tecla la cancela); `y`/Enter copia (sin selección
//! copia la línea del cursor) y sale; Esc/`q` sale sin copiar.

use base64::Engine;
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use friki_term::{Pos, Terminal};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// Máximo de bytes por copia.
pub const MAX_COPY: usize = 1 << 20;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CopyOutcome {
    Continue,
    Cancel,
    Copy(String),
}

/// Por dónde ha ido la copia.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Clipboard {
    /// Portapapeles del sistema local (arboard).
    System,
    /// Secuencia OSC 52 que el llamador debe escribir a la terminal anfitriona.
    Osc52(String),
}

/// Copia con el portapapeles del sistema y, si falla, devuelve OSC 52.
pub fn copy_to_clipboard(text: &str) -> Result<Clipboard, String> {
    copy_with(text, |t| {
        arboard::Clipboard::new()
            .and_then(|mut c| c.set_text(t))
            .map_err(|e| e.to_string())
    })
}

/// Igual con el portapapeles del sistema inyectado (pruebas sin portapapeles real).
pub fn copy_with(
    text: &str,
    system: impl FnOnce(&str) -> Result<(), String>,
) -> Result<Clipboard, String> {
    if text.len() > MAX_COPY {
        return Err(format!(
            "selección demasiado grande (máx. {MAX_COPY} bytes)"
        ));
    }
    Ok(match system(text) {
        Ok(()) => Clipboard::System,
        Err(_) => Clipboard::Osc52(osc52(text)),
    })
}

fn osc52(text: &str) -> String {
    let b64 = base64::engine::general_purpose::STANDARD.encode(text);
    format!("\x1b]52;c;{b64}\x07")
}

/// Celda de una fila: (columna de inicio, ancho, carácter).
type RowCell = (u16, u16, char);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Class {
    Space,
    Word,
    Punct,
}

fn class(c: char) -> Class {
    if c.is_whitespace() {
        Class::Space
    } else if c.is_alphanumeric() || c == '_' {
        Class::Word
    } else {
        Class::Punct
    }
}

pub struct CopyMode {
    cursor: Pos,
    /// Origen de la selección y si es rectangular.
    anchor: Option<(Pos, bool)>,
    rows: u16,
    cols: u16,
}

impl CopyMode {
    pub fn new(term: &Terminal) -> Self {
        let s = term.screen();
        let row = term.scrollback_len() + usize::from(s.cursor.0);
        let mut m = Self {
            cursor: (row, s.cursor.1),
            anchor: None,
            rows: s.rows,
            cols: s.cols,
        };
        m.cursor.1 = m.snap(&m.row_cells(term, row), m.cursor.1);
        m
    }

    #[cfg(test)]
    pub fn cursor(&self) -> Pos {
        self.cursor
    }

    /// `Some(rectangular)` si hay selección en curso.
    pub fn selecting(&self) -> Option<bool> {
        self.anchor.map(|(_, b)| b)
    }

    fn last_row(&self, term: &Terminal) -> usize {
        (term.scrollback_len() + usize::from(self.rows)).saturating_sub(1)
    }

    // ponytail: cada fila se lee con `text_range` (clona la pantalla); basta para
    // teclado, si hiciera falta velocidad se cachea la fila del cursor.
    fn row_cells(&self, term: &Terminal, row: usize) -> Vec<RowCell> {
        let line = term.text_range((row, 0), (row, self.cols.saturating_sub(1)), false);
        let mut v = Vec::new();
        let mut c = 0u16;
        for ch in line.chars() {
            let w = ch.width().map_or(0, |w| w as u16);
            if w == 0 {
                continue;
            }
            if c + w > self.cols {
                break;
            }
            v.push((c, w, ch));
            c += w;
        }
        while c < self.cols {
            v.push((c, 1, ' '));
            c += 1;
        }
        v
    }

    /// Columna de inicio de la celda que contiene `col`.
    fn snap(&self, cells: &[RowCell], col: u16) -> u16 {
        cells.iter().rev().find(|c| c.0 <= col).map_or(0, |c| c.0)
    }

    fn go(&mut self, term: &mut Terminal, row: usize, col: u16) {
        let row = row.min(self.last_row(term));
        let cells = self.row_cells(term, row);
        self.cursor = (row, self.snap(&cells, col));
        let top = term.top_row();
        if row < top || row >= top + usize::from(self.rows) {
            term.reveal_row(row);
        }
    }

    fn word_starts(cells: &[RowCell]) -> Vec<u16> {
        (0..cells.len())
            .filter(|&i| {
                class(cells[i].2) != Class::Space
                    && (i == 0 || class(cells[i - 1].2) != class(cells[i].2))
            })
            .map(|i| cells[i].0)
            .collect()
    }

    fn next_word(&self, term: &Terminal) -> Pos {
        let (row, col) = self.cursor;
        let here = Self::word_starts(&self.row_cells(term, row));
        if let Some(&c) = here.iter().find(|&&c| c > col) {
            return (row, c);
        }
        for r in row + 1..=self.last_row(term) {
            if let Some(&c) = Self::word_starts(&self.row_cells(term, r)).first() {
                return (r, c);
            }
        }
        self.cursor
    }

    fn prev_word(&self, term: &Terminal) -> Pos {
        let (row, col) = self.cursor;
        let here = Self::word_starts(&self.row_cells(term, row));
        if let Some(&c) = here.iter().rev().find(|&&c| c < col) {
            return (row, c);
        }
        for r in (0..row).rev() {
            if let Some(&c) = Self::word_starts(&self.row_cells(term, r)).last() {
                return (r, c);
            }
        }
        self.cursor
    }

    fn copied_text(&self, term: &Terminal) -> String {
        match self.anchor {
            Some((a, block)) => term.text_range(a, self.cursor, block),
            None => term.text_range(
                (self.cursor.0, 0),
                (self.cursor.0, self.cols.saturating_sub(1)),
                false,
            ),
        }
    }

    pub fn handle_key(&mut self, key: &KeyEvent, term: &mut Terminal) -> CopyOutcome {
        if key.kind == KeyEventKind::Release {
            return CopyOutcome::Continue;
        }
        let (row, col) = self.cursor;
        let page = usize::from(self.rows.saturating_sub(1).max(1));
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => return CopyOutcome::Cancel,
            KeyCode::Enter | KeyCode::Char('y') => {
                return CopyOutcome::Copy(self.copied_text(term));
            }
            KeyCode::Char('v') if ctrl => self.toggle(true),
            KeyCode::Char('v') => self.toggle(false),
            KeyCode::Left | KeyCode::Char('h') => {
                let cells = self.row_cells(term, row);
                let i = cells.iter().rposition(|c| c.0 < col).unwrap_or(0);
                self.go(term, row, cells[i].0);
            }
            KeyCode::Right | KeyCode::Char('l') => {
                let cells = self.row_cells(term, row);
                let next = cells.iter().find(|c| c.0 > col).map_or(col, |c| c.0);
                self.go(term, row, next);
            }
            KeyCode::Up | KeyCode::Char('k') => self.go(term, row.saturating_sub(1), col),
            KeyCode::Down | KeyCode::Char('j') => self.go(term, row + 1, col),
            KeyCode::PageUp => self.go(term, row.saturating_sub(page), col),
            KeyCode::PageDown => self.go(term, row + page, col),
            KeyCode::Char('0') | KeyCode::Home => self.go(term, row, 0),
            KeyCode::Char('$') | KeyCode::End => {
                let cells = self.row_cells(term, row);
                let last = cells.iter().rfind(|c| c.2 != ' ').map_or(0, |c| c.0);
                self.go(term, row, last);
            }
            KeyCode::Char('g') => self.go(term, 0, 0),
            KeyCode::Char('G') => self.go(term, self.last_row(term), 0),
            KeyCode::Char('w') => {
                let (r, c) = self.next_word(term);
                self.go(term, r, c);
            }
            KeyCode::Char('b') => {
                let (r, c) = self.prev_word(term);
                self.go(term, r, c);
            }
            _ => {}
        }
        CopyOutcome::Continue
    }

    /// `v`/Ctrl+v: inicia la selección, la cancela si es del mismo tipo o cambia de tipo.
    fn toggle(&mut self, block: bool) {
        self.anchor = match self.anchor {
            Some((_, b)) if b == block => None,
            Some((a, _)) => Some((a, block)),
            None => Some((self.cursor, block)),
        };
    }

    /// Columnas [lo, hi] seleccionadas en `row` (sin ajustar a caracteres anchos).
    fn selected_cols(&self, row: usize) -> Option<(u16, u16)> {
        let (anchor, block) = self.anchor?;
        let (a, b) = if anchor <= self.cursor {
            (anchor, self.cursor)
        } else {
            (self.cursor, anchor)
        };
        if row < a.0 || row > b.0 {
            return None;
        }
        Some(if block {
            (anchor.1.min(self.cursor.1), anchor.1.max(self.cursor.1))
        } else {
            (
                if row == a.0 { a.1 } else { 0 },
                if row == b.0 { b.1 } else { self.cols - 1 },
            )
        })
    }

    pub fn render(&self, frame: &mut Frame, area: Rect, term: &Terminal) {
        if area.is_empty() {
            return;
        }
        let screen = term.screen();
        let top = term.top_row();
        let buf = frame.buffer_mut();
        let sel = Style::default().fg(Color::White).bg(Color::Blue);
        let wide = |y: u16, c: u16| {
            screen
                .cells
                .get(usize::from(y) * usize::from(screen.cols) + usize::from(c))
                .is_some_and(|x| x.text.width() == 2)
        };
        let height = area.height.min(screen.rows);
        for y in 0..height {
            let Some((mut lo, mut hi)) = self.selected_cols(top + usize::from(y)) else {
                continue;
            };
            // Incluir caracteres anchos enteros.
            if lo > 0 && wide(y, lo - 1) {
                lo -= 1;
            }
            if wide(y, hi) {
                hi += 1;
            }
            let x0 = area.x.saturating_add(lo);
            let x1 = area.x.saturating_add(hi + 1).min(area.right());
            if x0 < x1 {
                buf.set_style(Rect::new(x0, area.y + y, x1 - x0, 1), sel);
            }
        }
        // Cursor.
        let (crow, ccol) = self.cursor;
        if crow >= top && crow - top < usize::from(height) {
            let y = u16::try_from(crow - top).unwrap_or(0);
            let w = if wide(y, ccol) { 2 } else { 1 };
            let x = area.x.saturating_add(ccol);
            let end = x.saturating_add(w).min(area.right());
            if x < end {
                buf.set_style(
                    Rect::new(x, area.y + y, end - x, 1),
                    Style::default().add_modifier(Modifier::REVERSED),
                );
            }
        }
        // Estado, a la derecha de la última fila.
        let mode = match self.selecting() {
            Some(true) => "BLOQUE",
            Some(false) => "VISUAL",
            None => "COPIA",
        };
        let info = format!(" {mode} {}/{} ", self.cursor.0 + 1, self.last_row(term) + 1);
        let w = u16::try_from(info.width())
            .unwrap_or(area.width)
            .min(area.width);
        buf.set_stringn(
            area.right() - w,
            area.bottom() - 1,
            &info,
            usize::from(w),
            Style::default().fg(Color::Black).bg(Color::Yellow),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal as RTerm;
    use ratatui::backend::TestBackend;

    fn key(c: KeyCode) -> KeyEvent {
        KeyEvent::new(c, KeyModifiers::NONE)
    }

    fn press(m: &mut CopyMode, t: &mut Terminal, keys: &str) -> CopyOutcome {
        let mut out = CopyOutcome::Continue;
        for c in keys.chars() {
            out = m.handle_key(&key(KeyCode::Char(c)), t);
        }
        out
    }

    /// 4 filas x 20 cols; "uno dos tres", "cuatro cinco", "世界ab cd" y cursor al final.
    fn term() -> Terminal {
        let mut t = Terminal::new(4, 20, 50);
        t.process("uno dos tres\r\ncuatro cinco\r\n世界ab cd\r\nfin".as_bytes());
        t
    }

    #[test]
    fn starts_at_remote_cursor_and_moves_in_bounds() {
        let mut t = term();
        let mut m = CopyMode::new(&t);
        assert_eq!(m.cursor(), (3, 3));
        press(&mut m, &mut t, "jjjj");
        assert_eq!(m.cursor().0, 3); // borde inferior
        press(&mut m, &mut t, "kkkkkk");
        assert_eq!(m.cursor().0, 0); // borde superior
        press(&mut m, &mut t, "0h");
        assert_eq!(m.cursor(), (0, 0));
        m.handle_key(&key(KeyCode::Left), &mut t);
        assert_eq!(m.cursor(), (0, 0));
        press(&mut m, &mut t, "$");
        assert_eq!(m.cursor(), (0, 11));
        press(&mut m, &mut t, "llllllllllllllllllllllll");
        assert_eq!(m.cursor(), (0, 19)); // borde derecho
        m.handle_key(&key(KeyCode::Right), &mut t);
        assert_eq!(m.cursor(), (0, 19));
    }

    #[test]
    fn g_and_big_g_and_pages() {
        let mut t = Terminal::new(3, 10, 50);
        for i in 0..30 {
            t.process(format!("l{i}\r\n").as_bytes());
        }
        let mut m = CopyMode::new(&t);
        press(&mut m, &mut t, "g");
        assert_eq!(m.cursor(), (0, 0));
        assert_eq!(t.top_row(), 0); // la vista sigue al cursor
        m.handle_key(&key(KeyCode::PageDown), &mut t);
        assert_eq!(m.cursor().0, 2);
        m.handle_key(&key(KeyCode::PageDown), &mut t);
        m.handle_key(&key(KeyCode::PageUp), &mut t);
        assert_eq!(m.cursor().0, 2);
        press(&mut m, &mut t, "G");
        assert_eq!(m.cursor().0, t.scrollback_len() + 2);
        assert_eq!(t.scroll_offset(), 0);
    }

    #[test]
    fn words_forward_and_back() {
        let mut t = term();
        let mut m = CopyMode::new(&t);
        press(&mut m, &mut t, "g");
        press(&mut m, &mut t, "w");
        assert_eq!(m.cursor(), (0, 4));
        press(&mut m, &mut t, "w");
        assert_eq!(m.cursor(), (0, 8));
        press(&mut m, &mut t, "w");
        assert_eq!(m.cursor(), (1, 0)); // salta de línea
        press(&mut m, &mut t, "b");
        assert_eq!(m.cursor(), (0, 8));
        press(&mut m, &mut t, "bbb");
        assert_eq!(m.cursor(), (0, 0)); // no pasa del principio
    }

    #[test]
    fn normal_selection_text() {
        let mut t = term();
        let mut m = CopyMode::new(&t);
        press(&mut m, &mut t, "g");
        press(&mut m, &mut t, "wv");
        assert_eq!(m.selecting(), Some(false));
        press(&mut m, &mut t, "j");
        assert_eq!(
            press(&mut m, &mut t, "y"),
            CopyOutcome::Copy("dos tres\ncuatr".into())
        );
    }

    #[test]
    fn block_selection_and_toggle() {
        let mut t = term();
        let mut m = CopyMode::new(&t);
        press(&mut m, &mut t, "g");
        m.handle_key(
            &KeyEvent::new(KeyCode::Char('v'), KeyModifiers::CONTROL),
            &mut t,
        );
        assert_eq!(m.selecting(), Some(true));
        press(&mut m, &mut t, "jll");
        assert_eq!(
            m.handle_key(&key(KeyCode::Enter), &mut t),
            CopyOutcome::Copy("uno\ncua".into())
        );
        // `v` sobre bloque cambia a normal; otra vez `v` cancela.
        press(&mut m, &mut t, "v");
        assert_eq!(m.selecting(), Some(false));
        press(&mut m, &mut t, "v");
        assert_eq!(m.selecting(), None);
    }

    #[test]
    fn wide_chars() {
        let mut t = term();
        let mut m = CopyMode::new(&t);
        press(&mut m, &mut t, "gjj");
        assert_eq!(m.cursor(), (2, 0));
        press(&mut m, &mut t, "l");
        assert_eq!(m.cursor(), (2, 2)); // salta la continuación
        press(&mut m, &mut t, "h");
        assert_eq!(m.cursor(), (2, 0));
        // Selección normal sobre 世界: copia los caracteres enteros.
        press(&mut m, &mut t, "vl");
        assert_eq!(press(&mut m, &mut t, "y"), CopyOutcome::Copy("世界".into()));
        // Bloque que empieza a mitad de un ancho incluye el carácter entero.
        let mut m = CopyMode::new(&t);
        m.cursor = (2, 2);
        m.anchor = Some(((1, 1), true));
        assert_eq!(
            press(&mut m, &mut t, "y"),
            CopyOutcome::Copy("ua\n世界".into())
        );
    }

    #[test]
    fn no_selection_copies_line_and_cancel() {
        let mut t = term();
        let mut m = CopyMode::new(&t);
        press(&mut m, &mut t, "gj");
        assert_eq!(
            press(&mut m, &mut t, "y"),
            CopyOutcome::Copy("cuatro cinco".into())
        );
        assert_eq!(press(&mut m, &mut t, "q"), CopyOutcome::Cancel);
        assert_eq!(
            m.handle_key(&key(KeyCode::Esc), &mut t),
            CopyOutcome::Cancel
        );
    }

    #[test]
    fn clipboard_paths() {
        let ok = copy_with("hola", |t| {
            assert_eq!(t, "hola");
            Ok(())
        });
        assert_eq!(ok, Ok(Clipboard::System));
        let fallback = copy_with("hola", |_| Err("sin display".into()));
        assert_eq!(
            fallback,
            Ok(Clipboard::Osc52("\x1b]52;c;aG9sYQ==\x07".into()))
        );
        let big = "x".repeat(MAX_COPY + 1);
        assert!(copy_with(&big, |_| Ok(())).is_err());
        assert!(copy_with(&big[..MAX_COPY], |_| Ok(())).is_ok());
    }

    #[test]
    fn render_highlights_selection() {
        let mut t = term();
        let mut m = CopyMode::new(&t);
        press(&mut m, &mut t, "gjj");
        press(&mut m, &mut t, "vl");
        let mut rt = RTerm::new(TestBackend::new(20, 4)).unwrap();
        rt.draw(|f| m.render(f, f.area(), &t)).unwrap();
        let b = rt.backend().buffer();
        // Cursor en (2, 2) tras `l`; selección cols 0..=3 (世界 completos).
        assert_eq!(b[(0, 2)].bg, Color::Blue);
        assert_eq!(b[(3, 2)].bg, Color::Blue);
        assert_ne!(b[(4, 2)].bg, Color::Blue);
        assert_eq!(b[(0, 1)].bg, Color::Reset);
        let last: String = (0..20).map(|x| b[(x, 3)].symbol().to_owned()).collect();
        assert!(last.contains("VISUAL 3/"));
    }
}
