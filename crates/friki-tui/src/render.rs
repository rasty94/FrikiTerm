//! Dibujo: `Screen` -> buffer de ratatui, barra de estado y diálogos.

use friki_term::Screen;
use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};
use unicode_width::UnicodeWidthStr;

use crate::dialog::Dialog;
use crate::theme::Theme;

/// Copia el `Screen` al buffer, celda a celda, recortando a `area` y aplicando el tema.
pub fn screen_to_buffer(screen: &Screen, buf: &mut Buffer, area: Rect, theme: &Theme) {
    let cols = screen.cols as usize;
    for row in 0..screen.rows.min(area.height) {
        let mut prev_wide = false;
        for col in 0..screen.cols.min(area.width) {
            let Some(cell) = screen.cells.get(row as usize * cols + col as usize) else {
                continue;
            };
            let mut style = Style::default()
                .fg(theme.color(cell.fg, false))
                .bg(theme.color(cell.bg, true));
            for (on, m) in [
                (cell.bold, Modifier::BOLD),
                (cell.italic, Modifier::ITALIC),
                (cell.underline, Modifier::UNDERLINED),
                (cell.inverse, Modifier::REVERSED),
            ] {
                if on {
                    style = style.add_modifier(m);
                }
            }
            // Vacío = segunda mitad de un carácter ancho o celda en blanco.
            let sym = if cell.text.is_empty() {
                if prev_wide { "" } else { " " }
            } else {
                cell.text.as_str()
            };
            prev_wide = UnicodeWidthStr::width(sym) > 1;
            if let Some(dst) = buf.cell_mut((area.x + col, area.y + row)) {
                dst.set_symbol(sym).set_style(style);
            }
        }
    }
}

#[derive(Default)]
pub struct StatusInfo<'a> {
    pub session: &'a str,
    pub host: &'a str,
    pub state: &'a str,
    /// Cifrado negociado, compacto (vacío si no se conoce).
    pub cipher: &'a str,
    /// Redirecciones SSH activas, ya formateadas (vacío si no hay).
    pub forwards: &'a str,
    /// `legacy_algorithms` activo o algoritmo heredado negociado: indicador permanente.
    pub legacy: bool,
    /// Problema con el registro de sesión (no se abre o dejó de escribir).
    pub log_warn: Option<&'a str>,
}

pub fn status_line(info: &StatusInfo, right: &str, width: u16) -> Line<'static> {
    let mut left = format!(" {} | {} | {}", info.session, info.host, info.state);
    if !info.cipher.is_empty() {
        left.push_str(" | ");
        left.push_str(info.cipher);
    }
    if !info.forwards.is_empty() {
        left.push_str(" | ");
        left.push_str(info.forwards);
    }
    left.push(' ');
    let mut spans = vec![Span::raw(left)];
    if info.legacy {
        spans.push(Span::styled(
            " LEGACY ",
            Style::default()
                .fg(Color::White)
                .bg(Color::Red)
                .add_modifier(Modifier::BOLD),
        ));
    }
    if let Some(w) = info.log_warn {
        spans.push(Span::styled(
            format!(" {w} "),
            Style::default().fg(Color::Black).bg(Color::Yellow),
        ));
    }
    let used: usize = spans.iter().map(Span::width).sum();
    let pad = (width as usize).saturating_sub(used + right.width());
    spans.push(Span::raw(" ".repeat(pad)));
    spans.push(Span::raw(right.to_string()));
    Line::from(spans).style(Style::default().fg(Color::Black).bg(Color::Gray))
}

fn centered(area: Rect, w: u16, h: u16) -> Rect {
    let w = w.min(area.width);
    let h = h.min(area.height);
    let [_, v, _] = Layout::vertical([
        Constraint::Fill(1),
        Constraint::Length(h),
        Constraint::Fill(1),
    ])
    .areas(area);
    let [_, r, _] = Layout::horizontal([
        Constraint::Fill(1),
        Constraint::Length(w),
        Constraint::Fill(1),
    ])
    .areas(v);
    r
}

pub fn popup(frame: &mut Frame, title: &str, lines: Vec<Line<'static>>, alert: bool) {
    let area = frame.area();
    let width = lines
        .iter()
        .map(Line::width)
        .max()
        .unwrap_or(0)
        .max(title.width()) as u16
        + 4;
    let r = centered(
        area,
        width.min(area.width.saturating_sub(2)).max(10),
        lines.len() as u16 + 2,
    );
    let border = if alert {
        Style::default()
            .fg(Color::White)
            .bg(Color::Red)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::Cyan)
    };
    frame.render_widget(Clear, r);
    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!(" {title} "))
        .border_style(border);
    frame.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .block(block),
        r,
    );
}

/// Diálogo de conexión o aviso, centrado.
pub fn draw_dialog(frame: &mut Frame, d: &Dialog) {
    let mut lines: Vec<Line> = d.body.iter().map(|l| Line::from(l.clone())).collect();
    for i in 0..d.fields.len() {
        lines.push(Line::from(d.shown(i)));
    }
    popup(frame, &d.title, lines, d.alert);
}

#[cfg(test)]
mod tests {
    use super::*;
    use friki_term::{Cell, Color as TColor};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn cell(t: &str) -> Cell {
        Cell {
            text: t.into(),
            ..Cell::default()
        }
    }

    fn screen() -> Screen {
        let mut cells = vec![cell(" "); 4 * 2];
        cells[0] = Cell {
            text: "A".into(),
            fg: TColor::Indexed(1),
            bg: TColor::Rgb(1, 2, 3),
            bold: true,
            ..Cell::default()
        };
        cells[1] = Cell {
            text: "B".into(),
            inverse: true,
            underline: true,
            ..Cell::default()
        };
        cells[2] = cell("世");
        cells[3] = cell("");
        Screen {
            rows: 2,
            cols: 4,
            cells,
            cursor: (0, 1),
            cursor_visible: true,
            title: String::new(),
        }
    }

    #[test]
    fn screen_cells_colors_attrs() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 4, 2));
        screen_to_buffer(
            &screen(),
            &mut buf,
            Rect::new(0, 0, 4, 2),
            &Theme::default(),
        );
        let a = &buf[(0, 0)];
        assert_eq!(a.symbol(), "A");
        assert_eq!(a.fg, Color::Indexed(1));
        assert_eq!(a.bg, Color::Rgb(1, 2, 3));
        assert!(a.modifier.contains(Modifier::BOLD));
        let b = &buf[(1, 0)];
        assert!(
            b.modifier
                .contains(Modifier::REVERSED | Modifier::UNDERLINED)
        );
        assert_eq!(buf[(2, 0)].symbol(), "世");
        assert_eq!(buf[(3, 0)].symbol(), "");
    }

    #[test]
    fn status_line_shows_flags() {
        let info = StatusInfo {
            session: "web",
            host: "h:22",
            state: "conectado",
            cipher: "chacha20-poly1305",
            forwards: "L 127.0.0.1:1>x:2",
            legacy: true,
            log_warn: Some("sin registro"),
        };
        let line: String = status_line(&info, " ctrl+] q salir ", 120)
            .spans
            .iter()
            .map(|s| s.content.to_string())
            .collect();
        assert!(line.contains("web | h:22 | conectado"));
        assert!(line.contains("ctrl+] q salir"));
        assert!(line.contains("chacha20-poly1305") && line.contains("LEGACY"));
        assert!(line.contains("sin registro") && line.contains("L 127.0.0.1:1>x:2"));
    }

    #[test]
    fn theme_recolors_default_cells() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 4, 2));
        let (t, _) = Theme::named("solarized-dark");
        screen_to_buffer(&screen(), &mut buf, Rect::new(0, 0, 4, 2), &t);
        assert_eq!(buf[(1, 0)].bg, Color::Rgb(0, 0x2b, 0x36)); // fondo por defecto
        assert_eq!(buf[(0, 0)].fg, Color::Rgb(0xdc, 0x32, 0x2f)); // índice 1 remapeado
        assert_eq!(buf[(0, 0)].bg, Color::Rgb(1, 2, 3)); // RGB intacto
    }

    #[test]
    fn draw_dialog_shows_changed_key() {
        let mut t = Terminal::new(TestBackend::new(70, 14)).expect("terminal");
        let d = Dialog::changed_host("h", 22, "ssh-ed25519", "SHA256:new", "SHA256:old");
        t.draw(|f| draw_dialog(f, &d)).expect("draw");
        let text: String = t
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect();
        assert!(
            text.contains("HA CAMBIADO")
                && text.contains("SHA256:new")
                && text.contains("SHA256:old")
        );
    }
}
