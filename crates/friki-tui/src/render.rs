//! Dibujo: `Screen` -> buffer de ratatui, barra de estado y diálogos.

use friki_term::{Color as TColor, Screen};
use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};
use unicode_width::UnicodeWidthStr;

use crate::dialog::Dialog;

pub const HELP: [&str; 5] = [
    "Atajos (prefijo Ctrl+], como telnet):",
    "  Ctrl+] q        cerrar la sesión",
    "  Ctrl+] PgUp/PgDn  scrollback",
    "  Ctrl+] Ctrl+]   enviar un Ctrl+] al remoto",
    "  Ctrl+] ?        esta ayuda (cualquier tecla la cierra)",
];

fn color(c: TColor) -> Color {
    match c {
        TColor::Default => Color::Reset,
        TColor::Indexed(i) => Color::Indexed(i),
        TColor::Rgb(r, g, b) => Color::Rgb(r, g, b),
    }
}

/// Copia el `Screen` al buffer, celda a celda, recortando a `area`.
pub fn screen_to_buffer(screen: &Screen, buf: &mut Buffer, area: Rect) {
    let cols = screen.cols as usize;
    for row in 0..screen.rows.min(area.height) {
        let mut prev_wide = false;
        for col in 0..screen.cols.min(area.width) {
            let Some(cell) = screen.cells.get(row as usize * cols + col as usize) else {
                continue;
            };
            let mut style = Style::default().fg(color(cell.fg)).bg(color(cell.bg));
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

pub struct StatusInfo<'a> {
    pub session: &'a str,
    pub host: &'a str,
    pub state: &'a str,
}

pub fn status_line(info: &StatusInfo, width: u16) -> Line<'static> {
    let left = format!(" {} | {} | {} ", info.session, info.host, info.state);
    let right = "Ctrl+] q salir  PgUp/PgDn scroll  ? ayuda ";
    let pad = (width as usize).saturating_sub(left.width() + right.width());
    let style = Style::default().fg(Color::Black).bg(Color::Gray);
    Line::from(vec![
        Span::raw(left),
        Span::raw(" ".repeat(pad)),
        Span::raw(right),
    ])
    .style(style)
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

fn popup(frame: &mut Frame, title: &str, lines: Vec<Line<'static>>, alert: bool) {
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

pub fn draw(
    frame: &mut Frame,
    screen: Option<&Screen>,
    status: &StatusInfo,
    dialog: Option<&Dialog>,
    help: bool,
) {
    let area = frame.area();
    if area.height == 0 {
        return;
    }
    let [body, bar] = Layout::vertical([Constraint::Fill(1), Constraint::Length(1)]).areas(area);
    if let Some(s) = screen {
        screen_to_buffer(s, frame.buffer_mut(), body);
    }
    frame.render_widget(Paragraph::new(status_line(status, bar.width)), bar);
    if let Some(d) = dialog {
        let mut lines: Vec<Line> = d.body.iter().map(|l| Line::from(l.clone())).collect();
        for i in 0..d.fields.len() {
            lines.push(Line::from(d.shown(i)));
        }
        popup(frame, &d.title, lines, d.alert);
    } else if help {
        popup(
            frame,
            "Ayuda",
            HELP.iter().map(|l| Line::from(*l)).collect(),
            false,
        );
    } else if let Some(s) = screen {
        if s.cursor_visible && s.cursor.0 < body.height && s.cursor.1 < body.width {
            frame.set_cursor_position((body.x + s.cursor.1, body.y + s.cursor.0));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use friki_term::Cell;
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
        screen_to_buffer(&screen(), &mut buf, Rect::new(0, 0, 4, 2));
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
    fn draw_with_status_bar_and_cursor() {
        let mut t = Terminal::new(TestBackend::new(40, 3)).expect("terminal");
        let info = StatusInfo {
            session: "web",
            host: "h:22",
            state: "conectado",
        };
        t.draw(|f| draw(f, Some(&screen()), &info, None, false))
            .expect("draw");
        let buf = t.backend().buffer().clone();
        let row0: String = (0..4).map(|x| buf[(x, 0)].symbol().to_string()).collect();
        assert!(row0.starts_with("AB"));
        let last: String = (0..40).map(|x| buf[(x, 2)].symbol().to_string()).collect();
        assert!(last.contains("web | h:22 | conectado"));
        assert!(last.contains("Ctrl+] q"));
        t.backend_mut().assert_cursor_position((1, 0));
    }

    #[test]
    fn draw_dialog_and_help() {
        let mut t = Terminal::new(TestBackend::new(70, 14)).expect("terminal");
        let info = StatusInfo {
            session: "s",
            host: "h",
            state: "conectando",
        };
        let d = Dialog::changed_host("h", 22, "ssh-ed25519", "SHA256:new", "SHA256:old");
        t.draw(|f| draw(f, None, &info, Some(&d), false))
            .expect("draw");
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
        t.draw(|f| draw(f, None, &info, None, true)).expect("draw");
        let text: String = t
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect();
        assert!(text.contains("Ctrl+] q"));
    }
}
