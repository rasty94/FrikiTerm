//! Búsqueda en el scrollback de un panel (incremental, estilo vim `/`).
//!
//! Uso desde el bucle de la app:
//! - Al pulsar el atajo de búsqueda: `self.search = Some(Search::new(&term))`.
//! - Mientras `search` sea `Some`, TODAS las teclas del panel van a
//!   `search.handle_key(&key, &mut term)` (no al remoto). Si devuelve
//!   `SearchOutcome::Exit`, se pone `search = None` (con Esc la vista ya se ha
//!   restaurado; con `q` se queda en la coincidencia actual).
//! - En cada dibujo, después de pintar el `Screen` del panel:
//!   `search.render(frame, area_del_panel, &term)`. Pinta el resaltado y la línea
//!   inferior `/patrón  3/17  [Aa]` sobre la última fila del panel.
//!
//! Teclas: en modo escritura, texto/Backspace (búsqueda incremental), Enter pasa a
//! modo navegación; en navegación, Enter/`n` siguiente, `N`/Shift+Enter anterior
//! (con vuelta al principio/final), `/` vuelve a escribir. Alt+C alterna mayúsculas
//! en ambos modos. Esc sale restaurando la vista; `q` (navegación) sale dejándola.
//!
//! Las coincidencias se calculan al escribir; si llega salida nueva mientras se
//! busca no se recalculan (cierre y reabra la búsqueda).

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use friki_term::{Match, Terminal};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchOutcome {
    /// La búsqueda sigue abierta.
    Continue,
    /// Cerrar la búsqueda.
    Exit,
}

pub struct Search {
    text: String,
    case_sensitive: bool,
    matches: Vec<Match>,
    current: Option<usize>,
    /// Desplazamiento de la vista al abrir (para restaurar) y su fila superior.
    origin_offset: usize,
    origin_top: usize,
    typing: bool,
}

impl Search {
    pub fn new(term: &Terminal) -> Self {
        Self {
            text: String::new(),
            case_sensitive: false,
            matches: Vec::new(),
            current: None,
            origin_offset: term.scroll_offset(),
            origin_top: term.top_row(),
            typing: true,
        }
    }

    #[cfg(test)]
    pub fn text(&self) -> &str {
        &self.text
    }

    #[cfg(test)]
    pub fn matches(&self) -> &[Match] {
        &self.matches
    }

    #[cfg(test)]
    pub fn current(&self) -> Option<usize> {
        self.current
    }

    #[cfg(test)]
    pub fn case_sensitive(&self) -> bool {
        self.case_sensitive
    }

    fn restore(&self, term: &mut Terminal) {
        term.scroll_view(0);
        term.scroll_view(i32::try_from(self.origin_offset).unwrap_or(i32::MAX));
    }

    /// Recalcula coincidencias: la actual es la primera desde la vista de origen.
    fn refresh(&mut self, term: &mut Terminal) {
        self.matches = term.search(&self.text, self.case_sensitive);
        self.current = if self.matches.is_empty() {
            None
        } else {
            Some(
                self.matches
                    .iter()
                    .position(|m| m.row >= self.origin_top)
                    .unwrap_or(0),
            )
        };
        match self.current {
            Some(i) => term.reveal_row(self.matches[i].row),
            None => self.restore(term),
        }
    }

    fn step(&mut self, term: &mut Terminal, forward: bool) {
        let n = self.matches.len();
        let Some(i) = self.current else { return };
        let i = if forward {
            (i + 1) % n
        } else {
            (i + n - 1) % n
        };
        self.current = Some(i);
        term.reveal_row(self.matches[i].row);
    }

    pub fn handle_key(&mut self, key: &KeyEvent, term: &mut Terminal) -> SearchOutcome {
        if key.kind == KeyEventKind::Release {
            return SearchOutcome::Continue;
        }
        let (code, mods) = (key.code, key.modifiers);
        if code == KeyCode::Esc {
            self.restore(term);
            return SearchOutcome::Exit;
        }
        if mods.contains(KeyModifiers::ALT) && matches!(code, KeyCode::Char('c' | 'C')) {
            self.case_sensitive = !self.case_sensitive;
            self.refresh(term);
            return SearchOutcome::Continue;
        }
        let shift = mods.contains(KeyModifiers::SHIFT);
        if self.typing {
            match code {
                KeyCode::Enter => {
                    if self.current.is_some() {
                        self.typing = false;
                    }
                }
                KeyCode::Backspace => {
                    self.text.pop();
                    self.refresh(term);
                }
                KeyCode::Char(c) if !mods.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) => {
                    self.text.push(c);
                    self.refresh(term);
                }
                _ => {}
            }
            return SearchOutcome::Continue;
        }
        match code {
            KeyCode::Enter if shift => self.step(term, false),
            KeyCode::Enter | KeyCode::Char('n') => self.step(term, true),
            KeyCode::Char('N') => self.step(term, false),
            KeyCode::Char('/') => self.typing = true,
            KeyCode::Char('q') => return SearchOutcome::Exit,
            _ => {}
        }
        SearchOutcome::Continue
    }

    pub fn render(&self, frame: &mut Frame, area: Rect, term: &Terminal) {
        if area.is_empty() {
            return;
        }
        let top = term.top_row();
        let rows = usize::from(area.height);
        let buf = frame.buffer_mut();
        // Las coincidencias salen ordenadas por fila: solo las visibles.
        let first = self.matches.partition_point(|m| m.row < top);
        for (i, m) in self.matches.iter().enumerate().skip(first) {
            if m.row >= top + rows {
                break;
            }
            let x = area.x.saturating_add(m.start_col);
            let end = area.x.saturating_add(m.end_col).min(area.right());
            if x >= end {
                continue;
            }
            let style = if self.current == Some(i) {
                Style::default().fg(Color::White).bg(Color::Magenta)
            } else {
                Style::default().fg(Color::Black).bg(Color::Yellow)
            };
            let y = area.y + u16::try_from(m.row - top).unwrap_or(0);
            buf.set_style(Rect::new(x, y, end - x, 1), style);
        }
        let bar = Style::default().fg(Color::Black).bg(Color::Gray);
        let y = area.bottom() - 1;
        buf.set_style(Rect::new(area.x, y, area.width, 1), bar);
        let line = format!(
            "/{}  {}/{}  [{}]",
            self.text,
            self.current.map_or(0, |i| i + 1),
            self.matches.len(),
            if self.case_sensitive { "Aa" } else { "aa" }
        );
        let blank = " ".repeat(usize::from(area.width));
        buf.set_stringn(area.x, y, &blank, usize::from(area.width), bar);
        buf.set_stringn(area.x, y, &line, usize::from(area.width), bar);
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

    fn type_str(s: &mut Search, t: &mut Terminal, text: &str) {
        for c in text.chars() {
            s.handle_key(&key(KeyCode::Char(c)), t);
        }
    }

    /// 3 filas visibles, líneas "l00".."l19" con "foo" cada cinco.
    fn term() -> Terminal {
        let mut t = Terminal::new(3, 20, 100);
        for i in 0..20 {
            let extra = if i % 5 == 0 { " foo" } else { "" };
            t.process(format!("l{i:02}{extra} Foo\r\n").as_bytes());
        }
        t
    }

    #[test]
    fn incremental_and_case() {
        let mut t = term();
        let mut s = Search::new(&t);
        type_str(&mut s, &mut t, "fo");
        assert_eq!(s.matches().len(), 4 + 20); // "foo" y "Foo"
        type_str(&mut s, &mut t, "o");
        assert_eq!(s.matches().len(), 24);
        s.handle_key(
            &KeyEvent::new(KeyCode::Char('c'), KeyModifiers::ALT),
            &mut t,
        );
        assert!(s.case_sensitive());
        assert_eq!(s.matches().len(), 4);
        s.handle_key(&key(KeyCode::Backspace), &mut t);
        assert_eq!(s.text(), "fo");
        assert_eq!(s.matches().len(), 4);
    }

    /// Líneas "row00 x"; "needle" en las filas absolutas 0, 5, 10 y 15.
    fn term2() -> Terminal {
        let mut t = Terminal::new(3, 20, 100);
        for i in 0..20 {
            let extra = if i % 5 == 0 { "needle" } else { "x" };
            t.process(format!("row{i:02} {extra}\r\n").as_bytes());
        }
        t
    }

    fn rows(s: &Search) -> Vec<usize> {
        s.matches().iter().map(|m| m.row).collect()
    }

    #[test]
    fn navigation_wraps_and_reveals() {
        let mut t = term2();
        let mut s = Search::new(&t);
        type_str(&mut s, &mut t, "needle");
        assert_eq!(rows(&s), [0, 5, 10, 15]);
        // Origen al final: sin coincidencias después, vuelve a la primera.
        assert_eq!(s.current(), Some(0));
        assert_eq!(t.top_row(), 0);
        s.handle_key(&key(KeyCode::Enter), &mut t);
        s.handle_key(&key(KeyCode::Char('n')), &mut t);
        assert_eq!((s.current(), t.top_row()), (Some(1), 4));
        s.handle_key(&key(KeyCode::Enter), &mut t);
        assert_eq!(s.current(), Some(2));
        s.handle_key(&key(KeyCode::Char('N')), &mut t);
        s.handle_key(&key(KeyCode::Char('N')), &mut t);
        assert_eq!(s.current(), Some(0));
        s.handle_key(&KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT), &mut t);
        assert_eq!(s.current(), Some(3)); // vuelta al final
        s.handle_key(&key(KeyCode::Char('n')), &mut t);
        assert_eq!(s.current(), Some(0)); // vuelta al principio
    }

    #[test]
    fn starts_from_current_view() {
        let mut t = term2();
        t.scroll_view(t.scrollback_len() as i32 - 6); // fila superior = 6
        let mut s = Search::new(&t);
        type_str(&mut s, &mut t, "needle");
        assert_eq!(s.current(), Some(2)); // fila 10
    }

    #[test]
    fn esc_restores_view_q_keeps_it() {
        let mut t = term2();
        t.scroll_view(2);
        let (off, top) = (t.scroll_offset(), t.top_row());
        let mut s = Search::new(&t);
        type_str(&mut s, &mut t, "needle");
        assert_ne!(t.top_row(), top);
        assert_eq!(
            s.handle_key(&key(KeyCode::Esc), &mut t),
            SearchOutcome::Exit
        );
        assert_eq!((t.scroll_offset(), t.top_row()), (off, top));

        let mut s = Search::new(&t);
        type_str(&mut s, &mut t, "needle");
        s.handle_key(&key(KeyCode::Enter), &mut t);
        let moved = t.top_row();
        assert_eq!(
            s.handle_key(&key(KeyCode::Char('q')), &mut t),
            SearchOutcome::Exit
        );
        assert_eq!(t.top_row(), moved);
    }

    #[test]
    fn slash_returns_to_typing() {
        let mut t = term2();
        let mut s = Search::new(&t);
        type_str(&mut s, &mut t, "needle");
        s.handle_key(&key(KeyCode::Enter), &mut t);
        s.handle_key(&key(KeyCode::Char('/')), &mut t);
        type_str(&mut s, &mut t, "x");
        assert_eq!(s.text(), "needlex");
        assert!(s.matches().is_empty());
    }

    #[test]
    fn render_highlights_and_bar() {
        let mut t = term2();
        let mut s = Search::new(&t);
        type_str(&mut s, &mut t, "needle");
        let mut rt = RTerm::new(TestBackend::new(20, 3)).unwrap();
        rt.draw(|f| s.render(f, f.area(), &t)).unwrap();
        let buf = rt.backend().buffer();
        assert_eq!(buf[(6, 0)].bg, Color::Magenta);
        assert_eq!(buf[(11, 0)].bg, Color::Magenta);
        assert_ne!(buf[(12, 0)].bg, Color::Magenta);
        let bar: String = (0..20).map(|x| buf[(x, 2)].symbol().to_owned()).collect();
        assert_eq!(bar.trim_end(), "/needle  1/4  [aa]");
        // Una coincidencia no actual se resalta en otro color.
        s.handle_key(&key(KeyCode::Enter), &mut t);
        s.handle_key(&key(KeyCode::Char('n')), &mut t); // top = 4: fila 5 en y=1
        rt.draw(|f| s.render(f, f.area(), &t)).unwrap();
        assert_eq!(rt.backend().buffer()[(6, 1)].bg, Color::Magenta);
    }

    #[test]
    fn render_marks_other_matches_yellow() {
        let mut t = Terminal::new(3, 20, 0);
        t.process(b"ab ab\r\nab");
        let mut s = Search::new(&t);
        type_str(&mut s, &mut t, "ab");
        let mut rt = RTerm::new(TestBackend::new(20, 3)).unwrap();
        rt.draw(|f| s.render(f, f.area(), &t)).unwrap();
        let b = rt.backend().buffer();
        assert_eq!(b[(0, 0)].bg, Color::Magenta);
        assert_eq!(b[(3, 0)].bg, Color::Yellow);
        assert_eq!(b[(1, 1)].bg, Color::Yellow);
    }

    #[test]
    fn no_results_keeps_view() {
        let mut t = term();
        t.scroll_view(4);
        let mut s = Search::new(&t);
        type_str(&mut s, &mut t, "zzz");
        assert!(s.matches().is_empty());
        assert_eq!(s.current(), None);
        assert_eq!(t.scroll_offset(), 4);
        // Enter sin resultados no pasa a navegación.
        s.handle_key(&key(KeyCode::Enter), &mut t);
        type_str(&mut s, &mut t, "n");
        assert_eq!(s.text(), "zzzn");
    }
}
