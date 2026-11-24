//! Emulación de terminal y registro de sesión con enmascarado de contraseñas.
//! No conoce SSH ni la interfaz: entra `&[u8]`, sale `Screen`.
//!
//! Motor elegido: `vt100` 0.18. Cubre xterm-256color/VT220 (truecolor, cursor de
//! aplicación, bracketed paste, título OSC vía callback, caracteres anchos con
//! celda de continuación) y trae scrollback con vista desplazable. Se mantiene
//! activo y solo depende de `vte` y `unicode-width`. `alacritty_terminal` y
//! `termwiz` son más completos pero mucho más pesados y con API inestable; no
//! compensa para un cliente que solo necesita una rejilla de celdas.

use std::fs::File;
use std::io::Write;
use std::num::NonZeroU16;
use std::path::Path;

use friki_core::config::{BackspaceMode, Emulation, LogConfig, TerminalProfile};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Color {
    #[default]
    Default,
    Indexed(u8),
    Rgb(u8, u8, u8),
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Cell {
    /// Grafema completo (puede ser vacío en la segunda mitad de un carácter ancho).
    pub text: String,
    pub fg: Color,
    pub bg: Color,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub inverse: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Screen {
    pub rows: u16,
    pub cols: u16,
    /// Fila mayor: `cells[row * cols + col]`.
    pub cells: Vec<Cell>,
    /// (fila, columna).
    pub cursor: (u16, u16),
    pub cursor_visible: bool,
    pub title: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Char(char),
    Enter,
    Backspace,
    Tab,
    BackTab,
    Esc,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    Insert,
    Delete,
    F(u8),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Modifiers {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
}

/// Recoge el título OSC 0/2 que `vt100` solo expone por callback.
#[derive(Default)]
struct TitleCb {
    title: String,
}

impl vt100::Callbacks for TitleCb {
    fn set_window_title(&mut self, _: &mut vt100::Screen, title: &[u8]) {
        self.title = String::from_utf8_lossy(title).into_owned();
    }
}

fn nz(n: u16) -> NonZeroU16 {
    NonZeroU16::new(n).unwrap_or(NonZeroU16::MIN)
}

fn conv_color(c: vt100::Color) -> Color {
    match c {
        vt100::Color::Default => Color::Default,
        vt100::Color::Idx(i) => Color::Indexed(i),
        vt100::Color::Rgb(r, g, b) => Color::Rgb(r, g, b),
    }
}

/// Interpreta una combinación escrita como texto: modificadores `ctrl`, `alt`, `shift`
/// separados por `+` y una tecla final: un carácter (`"]"`, `"%"`, `"+"`), un nombre
/// (`enter`, `tab`, `backtab`, `esc`, `backspace`, `up`, `down`, `left`, `right`, `home`,
/// `end`, `pageup`, `pagedown`, `insert`, `delete`, `space`) o `f1`..`f12`. Sin distinguir
/// mayúsculas en nombres y modificadores. `None` si no es válida.
pub fn parse_chord(s: &str) -> Option<(Key, Modifiers)> {
    let (mods_part, key_part) = if s == "+" {
        ("", "+")
    } else if let Some(p) = s.strip_suffix("++") {
        // "ctrl++": modificadores + la tecla `+`. "++" a secas no es válida.
        if p.is_empty() {
            return None;
        }
        (p, "+")
    } else {
        match s.rsplit_once('+') {
            Some(("", _)) => return None,
            Some(p) => p,
            None => ("", s),
        }
    };
    let mut mods = Modifiers::default();
    if !mods_part.is_empty() {
        for m in mods_part.split('+') {
            match m.to_ascii_lowercase().as_str() {
                "ctrl" => mods.ctrl = true,
                "alt" => mods.alt = true,
                "shift" => mods.shift = true,
                _ => return None,
            }
        }
    }
    let lower = key_part.to_ascii_lowercase();
    let key = match lower.as_str() {
        "enter" => Key::Enter,
        "tab" => Key::Tab,
        "backtab" => Key::BackTab,
        "esc" => Key::Esc,
        "backspace" => Key::Backspace,
        "up" => Key::Up,
        "down" => Key::Down,
        "left" => Key::Left,
        "right" => Key::Right,
        "home" => Key::Home,
        "end" => Key::End,
        "pageup" => Key::PageUp,
        "pagedown" => Key::PageDown,
        "insert" => Key::Insert,
        "delete" => Key::Delete,
        "space" => Key::Char(' '),
        _ => {
            let mut it = key_part.chars();
            match (it.next(), it.next()) {
                (Some(c), None) => {
                    // Ctrl+letra llega siempre en minúscula desde el terminal.
                    let c = if mods.ctrl && c.is_ascii_alphabetic() {
                        c.to_ascii_lowercase()
                    } else {
                        c
                    };
                    Key::Char(c)
                }
                _ => {
                    let n: u8 = lower.strip_prefix('f')?.parse().ok()?;
                    if !(1..=12).contains(&n) || lower != format!("f{n}") {
                        return None;
                    }
                    Key::F(n)
                }
            }
        }
    };
    Some((key, mods))
}

/// Coincidencia de búsqueda. Filas en coordenadas absolutas del buffer: 0 es la línea más
/// antigua del scrollback y `scrollback_len() + rows - 1` la última de la pantalla.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Match {
    pub row: usize,
    /// Columnas [start, end) en celdas.
    pub start_col: u16,
    pub end_col: u16,
}

/// Posición absoluta (fila del buffer, columna) para selecciones.
pub type Pos = (usize, u16);

pub struct Terminal {
    parser: vt100::Parser<TitleCb>,
    emulation: Emulation,
    backspace: BackspaceMode,
    keymap: Vec<((Key, Modifiers), Vec<u8>)>,
    warnings: Vec<String>,
}

/// Lectura de filas absolutas sobre una copia de la pantalla (así `&self` no toca la vista
/// real). `set_scrollback` en la copia deja la fila pedida en la línea 0 visible.
struct Grid {
    s: vt100::Screen,
    len: usize,
    rows: usize,
    cols: u16,
}

impl Grid {
    fn new(s: &vt100::Screen) -> Self {
        Self {
            len: s.scrollback_len(),
            rows: usize::from(s.size().0.get()),
            cols: s.size().1.get(),
            s: s.clone(),
        }
    }

    fn total(&self) -> usize {
        self.len + self.rows
    }

    /// Sitúa `abs` en la fila visible devuelta.
    fn goto(&mut self, abs: usize) -> u16 {
        // `set_scrollback` cuesta O(filas): se reutiliza la ventana actual si contiene `abs`.
        let top = self.len - self.s.scrollback();
        if (top..top + self.rows).contains(&abs) {
            return u16::try_from(abs - top).unwrap_or(u16::MAX);
        }
        if abs < self.len {
            self.s.set_scrollback(self.len - abs);
            0
        } else {
            self.s.set_scrollback(0);
            u16::try_from(abs - self.len).unwrap_or(u16::MAX)
        }
    }

    /// Texto de la fila y, por byte, columna de inicio/fin (exclusivo) de su celda.
    /// Celdas vacías = espacio; se recortan las vacías del final.
    fn line(&mut self, abs: usize, lower: bool) -> (String, Vec<u16>, Vec<u16>) {
        let r = self.goto(abs);
        let mut last = 0;
        for c in 0..self.cols {
            if self.s.cell(r, c).is_some_and(|x| x.has_contents()) {
                last = c + 1;
            }
        }
        let (mut text, mut st, mut en) = (String::new(), Vec::new(), Vec::new());
        for c in 0..last {
            let Some(x) = self.s.cell(r, c) else { continue };
            if x.is_wide_continuation() {
                continue;
            }
            let w = if x.is_wide() { 2 } else { 1 };
            let t = if x.has_contents() { x.contents() } else { " " };
            if lower {
                text.extend(t.chars().flat_map(char::to_lowercase));
            } else {
                text.push_str(t);
            }
            st.resize(text.len(), c);
            en.resize(text.len(), c + w);
        }
        (text, st, en)
    }
}

impl Terminal {
    pub fn new(rows: u16, cols: u16, scrollback: usize) -> Self {
        Self::with_profile(
            rows,
            cols,
            &TerminalProfile {
                scrollback,
                ..TerminalProfile::default()
            },
        )
    }

    /// Igual que `new` con el perfil: scrollback, emulación (codificación de teclas),
    /// modo de retroceso y `keymap` (que `encode_key` consulta antes que nada; las
    /// entradas no válidas se ignoran y se informan en `profile_warnings`).
    pub fn with_profile(
        rows: u16,
        cols: u16,
        profile: &friki_core::config::TerminalProfile,
    ) -> Self {
        let mut keymap: Vec<((Key, Modifiers), Vec<u8>)> = Vec::new();
        let mut warnings = Vec::new();
        for (chord, bytes) in &profile.keymap {
            let Some(k) = parse_chord(chord) else {
                warnings.push(format!("keymap: tecla no válida {chord:?}"));
                continue;
            };
            match friki_core::config::parse_key_bytes(bytes) {
                Ok(b) => match keymap.iter_mut().find(|(kk, _)| *kk == k) {
                    Some(e) => e.1 = b,
                    None => keymap.push((k, b)),
                },
                Err(e) => warnings.push(format!("keymap {chord:?}: {e}")),
            }
        }
        Self {
            parser: vt100::Parser::new_with_callbacks(
                nz(rows),
                nz(cols),
                profile.scrollback,
                TitleCb::default(),
            ),
            emulation: profile.emulation,
            backspace: profile.backspace,
            keymap,
            warnings,
        }
    }

    /// Entradas del `keymap` del perfil que no se pudieron aplicar (tecla o bytes inválidos).
    pub fn profile_warnings(&self) -> Vec<String> {
        self.warnings.clone()
    }

    /// Valor de `TERM` que se anuncia al remoto para la emulación.
    pub fn term_name(emulation: friki_core::config::Emulation) -> &'static str {
        match emulation {
            Emulation::Xterm => "xterm-256color",
            Emulation::Vt100 => "vt100",
            Emulation::Vt220 => "vt220",
            Emulation::Linux => "linux",
            Emulation::Ansi => "ansi",
        }
    }

    /// Busca `needle` en scrollback + pantalla, de la más antigua a la más reciente.
    /// `case_sensitive = false` compara sin mayúsculas (Unicode simple). Cadena vacía = nada.
    pub fn search(&self, needle: &str, case_sensitive: bool) -> Vec<Match> {
        if needle.is_empty() {
            return Vec::new();
        }
        let needle: String = if case_sensitive {
            needle.to_owned()
        } else {
            needle.chars().flat_map(char::to_lowercase).collect()
        };
        let mut g = Grid::new(self.parser.screen());
        let mut out = Vec::new();
        for row in 0..g.total() {
            let (text, st, en) = g.line(row, !case_sensitive);
            for (i, m) in text.match_indices(needle.as_str()) {
                out.push(Match {
                    row,
                    start_col: st[i],
                    end_col: en[i + m.len() - 1],
                });
            }
        }
        out
    }

    /// Ajusta la vista para que `row` (absoluta) quede visible, centrada si se puede
    /// (acotada a [0, scrollback_len]). Filas fuera de rango = la última pantalla. En
    /// pantalla alternativa no hay scrollback (`scrollback_len() == 0`): no hace nada.
    pub fn reveal_row(&mut self, row: usize) {
        let len = self.parser.screen().scrollback_len();
        let rows = usize::from(self.parser.screen().size().0.get());
        let top = row.saturating_sub(rows / 2).min(len);
        self.parser.screen_mut().set_scrollback(len - top);
    }

    /// Fila absoluta que se ve en la línea 0 de la pantalla con el desplazamiento actual.
    pub fn top_row(&self) -> usize {
        let s = self.parser.screen();
        s.scrollback_len().saturating_sub(s.scrollback())
    }

    /// Texto entre dos posiciones absolutas (inclusive, en cualquier orden). Une líneas con
    /// `\n` salvo las que el remoto partió por ancho (wrap), quita espacios finales de cada
    /// línea y no corta caracteres anchos. `block = true`: selección rectangular.
    pub fn text_range(&self, from: Pos, to: Pos, block: bool) -> String {
        let mut g = Grid::new(self.parser.screen());
        let last_row = g.total() - 1;
        let last_col = g.cols - 1;
        let (a, b) = if from <= to { (from, to) } else { (to, from) };
        let (r0, r1) = (a.0.min(last_row), b.0.min(last_row));
        let (ca, cb) = (a.1.min(last_col), b.1.min(last_col));
        let mut out = String::new();
        for row in r0..=r1 {
            let (lo, hi) = if block {
                (ca.min(cb), ca.max(cb))
            } else {
                (
                    if row == r0 { ca } else { 0 },
                    if row == r1 { cb } else { last_col },
                )
            };
            let r = g.goto(row);
            // Empezar en la mitad de un ancho: incluir el carácter entero.
            let lo = if lo > 0 && g.s.cell(r, lo).is_some_and(|x| x.is_wide_continuation()) {
                lo - 1
            } else {
                lo
            };
            let mut line = String::new();
            for c in lo..=hi {
                match g.s.cell(r, c) {
                    Some(x) if x.is_wide_continuation() => {}
                    Some(x) if x.has_contents() => line.push_str(x.contents()),
                    _ => line.push(' '),
                }
            }
            let wrapped = !block && row != r1 && g.s.row_wrapped(r);
            out.push_str(if wrapped {
                &line
            } else {
                line.trim_end_matches(' ')
            });
            if row != r1 && !wrapped {
                out.push('\n');
            }
        }
        out
    }

    pub fn process(&mut self, bytes: &[u8]) {
        self.parser.process(bytes);
    }

    pub fn resize(&mut self, rows: u16, cols: u16) {
        self.parser.set_size(nz(rows), nz(cols));
    }

    /// Desplaza la vista del scrollback (positivo = hacia arriba). 0 vuelve al final.
    pub fn scroll_view(&mut self, lines: i32) {
        let s = self.parser.screen_mut();
        if lines == 0 {
            s.set_scrollback(0);
            return;
        }
        let target = (s.scrollback() as i64 + i64::from(lines)).max(0);
        s.set_scrollback(usize::try_from(target).unwrap_or(0));
    }

    /// Líneas que la vista está desplazada hacia atrás (0 = al final).
    pub fn scroll_offset(&self) -> usize {
        self.parser.screen().scrollback()
    }

    /// Líneas de historial disponibles.
    pub fn scrollback_len(&self) -> usize {
        self.parser.screen().scrollback_len()
    }

    /// El remoto está en la pantalla alternativa (vim, less...; sin scrollback).
    pub fn alternate_screen(&self) -> bool {
        self.parser.screen().alternate_screen()
    }

    /// `true` si el texto tiene varias líneas y el remoto no activó bracketed
    /// paste: la TUI debe pedir confirmación antes de enviarlo.
    pub fn paste_needs_confirmation(&self, text: &str) -> bool {
        text.trim_end_matches(['\n', '\r']).contains(['\n', '\r'])
            && !self.parser.screen().bracketed_paste()
    }

    pub fn screen(&self) -> Screen {
        let s = self.parser.screen();
        let (rows, cols) = (s.size().0.get(), s.size().1.get());
        let mut cells = Vec::with_capacity(usize::from(rows) * usize::from(cols));
        for r in 0..rows {
            for c in 0..cols {
                cells.push(match s.cell(r, c) {
                    Some(x) if !x.is_wide_continuation() => Cell {
                        text: x.contents().to_owned(),
                        fg: conv_color(x.fgcolor()),
                        bg: conv_color(x.bgcolor()),
                        bold: x.bold(),
                        italic: x.italic(),
                        underline: x.underline(),
                        inverse: x.inverse(),
                    },
                    // Segunda mitad de un carácter ancho: texto vacío, mismos colores.
                    Some(x) => Cell {
                        fg: conv_color(x.fgcolor()),
                        bg: conv_color(x.bgcolor()),
                        ..Cell::default()
                    },
                    None => Cell::default(),
                });
            }
        }
        Screen {
            rows,
            cols,
            cells,
            cursor: s.cursor_position(),
            cursor_visible: !s.hide_cursor() && s.scrollback() == 0,
            title: (self.parser.callbacks().title.chars())
                .filter(|c| !c.is_control())
                .collect(),
        }
    }

    /// Respeta el modo cursor de aplicación y el keypad.
    pub fn encode_key(&self, key: Key, mods: Modifiers) -> Vec<u8> {
        if let Some((_, b)) = self.keymap.iter().find(|(k, _)| *k == (key, mods)) {
            return b.clone();
        }
        let app = self.parser.screen().application_cursor();
        let e = self.emulation;
        // Solo xterm codifica modificadores en CSI (`ESC[1;5A`); el resto los ignora.
        let m = if e == Emulation::Xterm {
            1 + u8::from(mods.shift) + 2 * u8::from(mods.alt) + 4 * u8::from(mods.ctrl)
        } else {
            1
        };
        let csi = |n: &str, fin: char| {
            if m == 1 {
                format!("\x1b[{n}{fin}")
            } else {
                format!("\x1b[{};{m}{fin}", if n.is_empty() { "1" } else { n })
            }
        };
        let cursor = |fin: char| {
            if m == 1 && app {
                format!("\x1bO{fin}")
            } else {
                csi("", fin)
            }
        };
        let tilde = |n: &str| csi(n, '~');
        // Teclas que producen un byte: Alt = prefijo ESC.
        let alt = |b: Vec<u8>| {
            if mods.alt {
                [vec![0x1b], b].concat()
            } else {
                b
            }
        };
        match key {
            Key::Char(c) => {
                let mut b = [0u8; 4];
                let mut out = c.encode_utf8(&mut b).as_bytes().to_vec();
                if mods.ctrl {
                    match c {
                        'a'..='z' | 'A'..='Z' => out = vec![(c as u8) & 0x1f],
                        '@' | ' ' => out = vec![0],
                        '[' => out = vec![0x1b],
                        '\\' => out = vec![0x1c],
                        ']' => out = vec![0x1d],
                        '^' => out = vec![0x1e],
                        '_' | '/' => out = vec![0x1f],
                        '?' => out = vec![0x7f],
                        _ => {}
                    }
                }
                return alt(out);
            }
            Key::Enter => return alt(vec![b'\r']),
            Key::Backspace => {
                return alt(vec![match self.backspace {
                    BackspaceMode::Delete => 0x7f,
                    BackspaceMode::CtrlH => 0x08,
                }]);
            }
            Key::Tab => return alt(vec![b'\t']),
            Key::Esc => return alt(vec![0x1b]),
            _ => {}
        }
        // Secuencias por emulación (fuente: `infocmp -x vt100 vt220 linux ansi` de ncurses):
        // - vt100: sin Home/End/Ins/Del/PgUp/PgDn/BackTab; F1-F4 = PF1-PF4 `ESC O P..S`
        //   (kf1..kf4); F5+ no existen.
        // - vt220: kfnd/kslt (Find/Select) = Home `ESC[1~`, End `ESC[4~`; kich1 `ESC[2~`,
        //   kdch1 `ESC[3~`, knp `ESC[6~`, kpp `ESC[5~`; kf1..kf4 `ESC O P..S`; kf6..kf12
        //   `ESC[17~`..`ESC[24~`; kf5 no está en terminfo, se usa el habitual `ESC[15~`.
        // - linux: kf1..kf5 `ESC[[A`..`ESC[[E`; kf6.. `ESC[17~`..; khome `ESC[1~`,
        //   kend `ESC[4~`, kcbt `ESC[Z`.
        // - ansi: khome `ESC[H`, kich1 `ESC[L`; el resto como xterm sin modificadores.
        use Emulation::{Ansi, Linux, Vt100, Vt220};
        match key {
            Key::BackTab if e == Vt100 => String::new(),
            Key::BackTab => "\x1b[Z".into(),
            Key::Up => cursor('A'),
            Key::Down => cursor('B'),
            Key::Right => cursor('C'),
            Key::Left => cursor('D'),
            Key::Home | Key::End | Key::Insert | Key::Delete | Key::PageUp | Key::PageDown
                if e == Vt100 =>
            {
                String::new()
            }
            Key::Home if matches!(e, Vt220 | Linux) => "\x1b[1~".into(),
            Key::End if matches!(e, Vt220 | Linux) => "\x1b[4~".into(),
            Key::Home => cursor('H'),
            Key::End => cursor('F'),
            Key::Insert if e == Ansi => "\x1b[L".into(),
            Key::Insert => tilde("2"),
            Key::Delete => tilde("3"),
            Key::PageUp => tilde("5"),
            Key::PageDown => tilde("6"),
            Key::F(n) if e == Vt100 && n >= 5 => String::new(),
            Key::F(n) if e == Linux && (1..=5).contains(&n) => {
                format!("\x1b[[{}", ['A', 'B', 'C', 'D', 'E'][usize::from(n - 1)])
            }
            Key::F(n @ 1..=4) => {
                // vt100/vt220 (y xterm sin modificadores) usan SS3.
                let fin = ['P', 'Q', 'R', 'S'][usize::from(n - 1)];
                if m == 1 {
                    format!("\x1bO{fin}")
                } else {
                    csi("", fin)
                }
            }
            Key::F(n @ 5..=12) => {
                tilde(["15", "17", "18", "19", "20", "21", "23", "24"][usize::from(n - 5)])
            }
            _ => String::new(),
        }
        .into_bytes()
    }

    /// Usa bracketed paste si el remoto lo activó. Se eliminan todos los
    /// controles (C0 salvo tab/LF/CR, DEL y C1) para que no puedan cerrar el modo
    /// (`ESC[201~`), enviar Ctrl+C ni inyectar secuencias.
    pub fn encode_paste(&self, text: &str) -> Vec<u8> {
        let clean: String = text
            .chars()
            .filter(|c| !c.is_control() || matches!(c, '\t' | '\n' | '\r'))
            .collect();
        if self.parser.screen().bracketed_paste() {
            format!("\x1b[200~{clean}\x1b[201~").into_bytes()
        } else {
            clean.into_bytes()
        }
    }
}

const PROMPTS: [&str; 14] = [
    "password",
    "passphrase",
    "passcode",
    "contraseña",
    "clave",
    "passwort",
    "kennwort",
    "mot de passe",
    "senha",
    "secret",
    "token",
    "verification code",
    "pin",
    "otp",
];

/// `pin` y `otp` solo como palabra completa (evita "shopping", "footprint"...).
fn is_password_prompt(line: &str) -> bool {
    let l = line.to_lowercase();
    PROMPTS.iter().any(|p| {
        l.match_indices(p).any(|(i, _)| {
            let word = |c: Option<char>| c.is_some_and(char::is_alphanumeric);
            (*p != "pin" && *p != "otp")
                || (!word(l[..i].chars().next_back()) && !word(l[i + p.len()..].chars().next()))
        })
    })
}

#[derive(Clone, Copy, PartialEq)]
enum St {
    Text,
    Esc,
    Csi,
    /// Cadena (OSC, DCS...) hasta BEL o ST.
    Str,
    StrEsc,
}

/// Quita secuencias de escape y controles (salvo `\t` y `\n`), byte a byte.
struct Filter(St);

impl Filter {
    fn feed(&mut self, b: u8, out: &mut Vec<u8>) {
        match self.0 {
            St::Text => match b {
                0x1b => self.0 = St::Esc,
                b'\t' | b'\n' | 0x20..=0x7e | 0x80..=0xff => out.push(b),
                _ => {}
            },
            St::Esc => {
                self.0 = match b {
                    b'[' => St::Csi,
                    b']' | b'P' | b'X' | b'^' | b'_' => St::Str,
                    0x20..=0x2f => St::Esc,
                    _ => St::Text,
                }
            }
            St::Csi => {
                if (0x40..=0x7e).contains(&b) {
                    self.0 = St::Text;
                }
            }
            St::Str => match b {
                0x07 => self.0 = St::Text,
                0x1b => self.0 = St::StrEsc,
                _ => {}
            },
            St::StrEsc => self.0 = if b == b'\\' { St::Text } else { St::Str },
        }
    }
}

/// Registro de sesión. La salida se registra siempre; la entrada solo si
/// `log_input`, y nunca la entrada tras un aviso de contraseña/passphrase visto
/// en la salida (se escribe `[entrada oculta]`) hasta el siguiente Enter.
///
/// Límite conocido: no se detecta una contraseña por falta de eco del remoto
/// (el eco llega de forma asíncrona respecto a la entrada, la heurística no es
/// fiable); solo se detecta por el texto del aviso.
pub struct SessionLogger {
    file: File,
    log_input: bool,
    out_filter: Filter,
    in_filter: Filter,
    /// Línea de salida en curso (ya sin secuencias de escape).
    line: Vec<u8>,
    /// Última línea de salida no vacía y terminada.
    last: Vec<u8>,
    /// Se ha visto un aviso desde el último Enter de la entrada.
    armed: bool,
    /// Ya se escribió el marcador de entrada oculta en esta línea.
    marked: bool,
}

/// Fecha local AAAA-MM-DD. `time` rechaza obtener el desfase local si hay
/// varios hilos (evita la carrera de `localtime_r`/`setenv`); en ese caso UTC.
fn today() -> String {
    let d = time::OffsetDateTime::now_local().unwrap_or_else(|_| time::OffsetDateTime::now_utc());
    format!("{:04}-{:02}-{:02}", d.year(), u8::from(d.month()), d.day())
}

/// Solo `[A-Za-z0-9._-]`; `.`, `..` y vacío pasan a `_`.
fn safe_name(s: &str) -> String {
    let r: String = s
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '_'
            }
        })
        .collect();
    if r.is_empty() || r == "." || r == ".." {
        "_".into()
    } else {
        r
    }
}

fn invalid(msg: &str) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidInput, msg.to_owned())
}

impl SessionLogger {
    /// Expande `%S`, `%H` y `%D` en `cfg.path`, relativo a `base_dir`. Rechaza
    /// rutas absolutas, con `..` o que acaben fuera de `base_dir`.
    pub fn open(
        cfg: &LogConfig,
        base_dir: &Path,
        session: &str,
        host: &str,
    ) -> std::io::Result<Self> {
        use std::path::Component;
        if !cfg
            .path
            .components()
            .all(|c| matches!(c, Component::Normal(_) | Component::CurDir))
        {
            return Err(invalid("ruta de log fuera del directorio base"));
        }
        let raw = cfg.path.to_string_lossy();
        let expanded = raw
            .replace("%S", &safe_name(session))
            .replace("%H", &safe_name(host))
            .replace("%D", &today());
        let path = base_dir.join(expanded);
        let Some(dir) = path.parent() else {
            return Err(invalid("ruta de log sin directorio"));
        };
        let mut db = std::fs::DirBuilder::new();
        db.recursive(true);
        #[cfg(unix)]
        std::os::unix::fs::DirBuilderExt::mode(&mut db, 0o700);
        db.create(dir)?;
        // Un enlace simbólico en un directorio intermedio no debe sacarnos de base_dir.
        if !dir.canonicalize()?.starts_with(base_dir.canonicalize()?) {
            return Err(invalid("ruta de log fuera del directorio base"));
        }
        let mut oo = std::fs::OpenOptions::new();
        oo.create(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            oo.mode(0o600).custom_flags(libc::O_NOFOLLOW);
        }
        if cfg.append {
            oo.append(true);
        } else {
            oo.write(true).truncate(true);
        }
        Ok(Self {
            file: oo.open(path)?,
            log_input: cfg.log_input,
            out_filter: Filter(St::Text),
            in_filter: Filter(St::Text),
            line: Vec::new(),
            last: Vec::new(),
            armed: false,
            marked: false,
        })
    }

    pub fn log_output(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        let mut out = Vec::with_capacity(bytes.len());
        for &b in bytes {
            let n = out.len();
            self.out_filter.feed(b, &mut out);
            if out.len() == n {
                continue;
            }
            if b == b'\n' {
                if !self.line.iter().all(u8::is_ascii_whitespace) {
                    self.last = std::mem::take(&mut self.line);
                }
                self.line.clear();
            } else {
                self.line.push(b);
            }
        }
        if self.line.len() > 512 {
            self.line.drain(..self.line.len() - 256);
        }
        // Aviso en la línea inacabada, o en la última no vacía (`Password:\r\n`).
        let ctx = if self.line.iter().all(u8::is_ascii_whitespace) {
            &self.last
        } else {
            &self.line
        };
        if is_password_prompt(&String::from_utf8_lossy(ctx)) {
            self.armed = true;
        }
        self.file.write_all(&out)
    }

    pub fn log_input(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        if !self.log_input {
            return Ok(());
        }
        let mut out = Vec::with_capacity(bytes.len());
        for &b in bytes {
            let enter = b == b'\r' || b == b'\n';
            if self.armed {
                if !self.marked {
                    self.marked = true;
                    out.extend_from_slice("[entrada oculta]".as_bytes());
                }
                if enter {
                    self.armed = false;
                    self.marked = false;
                    self.last.clear();
                    out.push(b'\n');
                }
            } else if enter {
                out.push(b'\n');
            } else {
                self.in_filter.feed(b, &mut out);
            }
        }
        self.file.write_all(&out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn term(bytes: &str) -> Screen {
        let mut t = Terminal::new(5, 20, 100);
        t.process(bytes.as_bytes());
        t.screen()
    }
    fn cell(s: &Screen, r: u16, c: u16) -> &Cell {
        &s.cells[usize::from(r * s.cols + c)]
    }
    fn none() -> Modifiers {
        Modifiers::default()
    }
    fn enc(t: &Terminal, k: Key) -> String {
        String::from_utf8(t.encode_key(k, none())).unwrap_or_default()
    }

    #[test]
    fn colores() {
        let s = term("\x1b[31mR\x1b[38;5;200mI\x1b[38;2;1;2;3;48;2;9;8;7mT\x1b[0;1;4mB");
        assert_eq!(cell(&s, 0, 0).fg, Color::Indexed(1));
        assert_eq!(cell(&s, 0, 1).fg, Color::Indexed(200));
        assert_eq!(cell(&s, 0, 2).fg, Color::Rgb(1, 2, 3));
        assert_eq!(cell(&s, 0, 2).bg, Color::Rgb(9, 8, 7));
        let b = cell(&s, 0, 3);
        assert!(b.bold && b.underline && b.fg == Color::Default);
    }

    #[test]
    fn cursor_y_borrado() {
        let s = term("hola\x1b[2;5H");
        assert_eq!(s.cursor, (1, 4));
        assert!(s.cursor_visible);
        let s = term("hola\x1b[2J\x1b[H\x1b[?25l");
        assert!(s.cells.iter().all(|c| c.text.is_empty()));
        assert!(!s.cursor_visible);
    }

    #[test]
    fn titulo() {
        assert_eq!(term("\x1b]0;mi titulo\x07").title, "mi titulo");
        assert_eq!(term("\x1b]2;otro\x1b\\").title, "otro");
    }

    #[test]
    fn anchos() {
        let s = term("漢a😀b");
        assert_eq!(cell(&s, 0, 0).text, "漢");
        assert_eq!(cell(&s, 0, 1).text, "");
        assert_eq!(cell(&s, 0, 2).text, "a");
        assert_eq!(cell(&s, 0, 3).text, "😀");
        assert_eq!(cell(&s, 0, 5).text, "b");
    }

    #[test]
    fn scrollback_y_resize() {
        let mut t = Terminal::new(2, 10, 50);
        t.process(b"uno\r\ndos\r\ntres\r\ncuatro");
        assert_eq!(t.screen().cells[0].text, "t");
        t.scroll_view(2);
        assert_eq!(t.screen().cells[0].text, "u");
        t.scroll_view(0);
        t.resize(3, 12);
        let s = t.screen();
        assert_eq!((s.rows, s.cols), (3, 12));
    }

    #[test]
    fn teclas_normal_y_aplicacion() {
        let mut t = Terminal::new(5, 20, 0);
        assert_eq!(enc(&t, Key::Up), "\x1b[A");
        assert_eq!(enc(&t, Key::Home), "\x1b[H");
        t.process(b"\x1b[?1h");
        assert_eq!(enc(&t, Key::Up), "\x1bOA");
        assert_eq!(enc(&t, Key::End), "\x1bOF");
        let ctrl = Modifiers {
            ctrl: true,
            ..none()
        };
        assert_eq!(t.encode_key(Key::Right, ctrl), b"\x1b[1;5C");
        assert_eq!(enc(&t, Key::F(1)), "\x1bOP");
        assert_eq!(enc(&t, Key::F(5)), "\x1b[15~");
        assert_eq!(enc(&t, Key::F(12)), "\x1b[24~");
        assert_eq!(enc(&t, Key::PageUp), "\x1b[5~");
        assert_eq!(enc(&t, Key::PageDown), "\x1b[6~");
        assert_eq!(enc(&t, Key::Insert), "\x1b[2~");
        assert_eq!(enc(&t, Key::Delete), "\x1b[3~");
        assert_eq!(enc(&t, Key::BackTab), "\x1b[Z");
        assert_eq!(enc(&t, Key::Enter), "\r");
        assert_eq!(enc(&t, Key::Backspace), "\x7f");
    }

    #[test]
    fn ctrl_y_alt() {
        let t = Terminal::new(5, 20, 0);
        let ctrl = Modifiers {
            ctrl: true,
            ..none()
        };
        let alt = Modifiers {
            alt: true,
            ..none()
        };
        assert_eq!(t.encode_key(Key::Char('c'), ctrl), vec![3]);
        assert_eq!(t.encode_key(Key::Char('['), ctrl), vec![0x1b]);
        assert_eq!(t.encode_key(Key::Char('x'), alt), b"\x1bx");
        assert_eq!(t.encode_key(Key::Char('é'), none()), "é".as_bytes());
    }

    #[test]
    fn pegado() {
        let mut t = Terminal::new(5, 20, 0);
        assert_eq!(t.encode_paste("ls\n"), b"ls\n");
        t.process(b"\x1b[?2004h");
        assert_eq!(t.encode_paste("ls"), b"\x1b[200~ls\x1b[201~");
        let inj = t.encode_paste("a\x1b[201~rm -rf /");
        assert_eq!(inj, b"\x1b[200~a[201~rm -rf /\x1b[201~");
        assert_eq!(inj.iter().filter(|&&b| b == 0x1b).count(), 2);
    }

    fn logger(dir: &Path, log_input: bool) -> SessionLogger {
        let cfg = LogConfig {
            path: "sub/%S-%H-%D.log".into(),
            append: false,
            log_input,
        };
        SessionLogger::open(&cfg, dir, "s1", "h1").unwrap()
    }
    fn tmp(n: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("friki-term-{n}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }
    fn read(d: &Path) -> String {
        let p = std::fs::read_dir(d.join("sub"))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        assert!(
            p.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("s1-h1-20")
        );
        std::fs::read_to_string(p).unwrap()
    }

    #[test]
    fn log_salida_sin_escapes() {
        let d = tmp("out");
        let mut l = logger(&d, false);
        l.log_output(b"\x1b[31mro").unwrap();
        l.log_output(b"jo\x1b[0m \x1b]0;t\x07ok\r\n").unwrap();
        l.log_input(b"secreto\r").unwrap();
        assert_eq!(read(&d), "rojo ok\n");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn log_entrada_oculta() {
        for prompt in [
            "Password: ",
            "Enter passphrase for key:",
            "Contraseña:",
            "Enter PIN",
            "Verification code: ",
            "OTP:",
        ] {
            let d = tmp("in");
            let mut l = logger(&d, true);
            l.log_output(b"login ok\r\n").unwrap();
            l.log_output(prompt.as_bytes()).unwrap();
            l.log_input(b"hun").unwrap();
            l.log_input(b"ter2\r").unwrap();
            l.log_output(b"\r\n$ ").unwrap();
            l.log_input(b"ls\r").unwrap();
            let r = read(&d);
            assert!(
                !r.contains("hunter2") && !r.contains("hun"),
                "{prompt}: {r}"
            );
            assert!(r.contains("[entrada oculta]"), "{prompt}");
            assert!(r.ends_with("ls\n"), "{prompt}: {r:?}");
            let _ = std::fs::remove_dir_all(&d);
        }
    }

    #[test]
    fn prompt_normal_no_oculta() {
        assert!(!is_password_prompt("user@host:~/shopping$ "));
        assert!(is_password_prompt("user@host's PASSWORD: "));
    }

    #[test]
    fn prompt_con_salto_de_linea() {
        let d = tmp("nl");
        let mut l = logger(&d, true);
        l.log_output(b"Password:\r\n").unwrap();
        l.log_input(b"hunter2\r").unwrap();
        l.log_output(b"$ ").unwrap();
        l.log_input(b"ls\r").unwrap();
        let r = read(&d);
        assert!(!r.contains("hunter2"), "{r:?}");
        assert!(r.ends_with("[entrada oculta]\n$ ls\n"), "{r:?}");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn entrada_antes_del_aviso_y_palabras_nuevas() {
        let d = tmp("pre");
        let mut l = logger(&d, true);
        l.log_input(b"abc").unwrap();
        l.log_output(b"Enter token: ").unwrap();
        l.log_input(b"hunter2\r").unwrap();
        assert!(!read(&d).contains("hunter2"));
        for p in ["Passcode:", "Clave:", "Kennwort:", "secret> "] {
            assert!(is_password_prompt(p), "{p}");
        }
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn entrada_filtrada() {
        let d = tmp("esc");
        let mut l = logger(&d, true);
        l.log_input(b"a\x1b]0;x\x07b\x07c\x1b[Ad\r").unwrap();
        assert_eq!(read(&d), "abcd\n");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn host_malicioso_y_rutas() {
        let d = tmp("path");
        let cfg = |p: &str| LogConfig {
            path: p.into(),
            append: true,
            log_input: false,
        };
        SessionLogger::open(&cfg("%H.log"), &d, "s", "a\n../b\x1b").unwrap();
        let names: Vec<_> = std::fs::read_dir(&d)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["a_.._b_.log"]);
        assert_eq!(safe_name(".."), "_");
        assert_eq!(safe_name(""), "_");
        for bad in ["/tmp/x.log", "../x.log", "a/../../x.log"] {
            let e = SessionLogger::open(&cfg(bad), &d, "s", "h").err().unwrap();
            assert_eq!(e.kind(), std::io::ErrorKind::InvalidInput, "{bad}");
        }
        let _ = std::fs::remove_dir_all(&d);
    }

    #[cfg(unix)]
    #[test]
    fn permisos_y_symlink() {
        use std::os::unix::fs::{MetadataExt, symlink};
        let d = tmp("perm");
        let _l = logger(&d, false);
        let mode = |p: &Path| std::fs::metadata(p).unwrap().mode() & 0o777;
        assert_eq!(mode(&d.join("sub")), 0o700);
        let f = std::fs::read_dir(d.join("sub")).unwrap().next().unwrap();
        assert_eq!(mode(&f.unwrap().path()), 0o600);
        // Enlace simbólico como fichero final: O_NOFOLLOW lo rechaza.
        let cfg = LogConfig {
            path: "ln.log".into(),
            append: true,
            log_input: false,
        };
        symlink(d.join("objetivo"), d.join("ln.log")).unwrap();
        assert!(SessionLogger::open(&cfg, &d, "s", "h").is_err());
        // Directorio enlazado hacia fuera de base_dir.
        let fuera = tmp("fuera");
        std::fs::create_dir_all(&fuera).unwrap();
        symlink(&fuera, d.join("d")).unwrap();
        let cfg = LogConfig {
            path: "d/x.log".into(),
            ..cfg
        };
        assert!(SessionLogger::open(&cfg, &d, "s", "h").is_err());
        let _ = std::fs::remove_dir_all(&d);
        let _ = std::fs::remove_dir_all(&fuera);
    }

    #[test]
    fn pegado_sin_controles() {
        let t = Terminal::new(5, 20, 0);
        assert_eq!(t.encode_paste("a\x03b\u{9b}c\x7fd\te\r\n"), b"abcd\te\r\n");
        assert!(t.paste_needs_confirmation("a\nb"));
        assert!(!t.paste_needs_confirmation("a\n"));
        assert!(!t.paste_needs_confirmation("a"));
        let mut t = t;
        t.process(b"\x1b[?2004h");
        assert!(!t.paste_needs_confirmation("a\nb"));
    }

    #[test]
    fn titulo_sin_c1() {
        assert_eq!(term("\x1b]0;a\u{9b}b\x01c\x07").title, "abc");
    }

    #[test]
    fn scroll_y_pantalla_alternativa() {
        let mut t = Terminal::new(2, 10, 50);
        t.process(b"uno\r\ndos\r\ntres\r\ncuatro");
        assert_eq!((t.scroll_offset(), t.scrollback_len()), (0, 2));
        t.scroll_view(1);
        assert_eq!(t.scroll_offset(), 1);
        t.scroll_view(99);
        assert_eq!(t.scroll_offset(), 2);
        assert!(!t.alternate_screen());
        t.process(b"\x1b[?1049h");
        assert!(t.alternate_screen());
    }

    fn mods(ctrl: bool, alt: bool, shift: bool) -> Modifiers {
        Modifiers { ctrl, alt, shift }
    }

    #[test]
    fn chord_validos() {
        let nm = Modifiers::default();
        for (s, k) in [
            ("enter", Key::Enter),
            ("tab", Key::Tab),
            ("backtab", Key::BackTab),
            ("esc", Key::Esc),
            ("backspace", Key::Backspace),
            ("up", Key::Up),
            ("down", Key::Down),
            ("left", Key::Left),
            ("right", Key::Right),
            ("home", Key::Home),
            ("end", Key::End),
            ("pageup", Key::PageUp),
            ("pagedown", Key::PageDown),
            ("insert", Key::Insert),
            ("delete", Key::Delete),
            ("space", Key::Char(' ')),
            ("f1", Key::F(1)),
            ("f12", Key::F(12)),
            ("]", Key::Char(']')),
            ("%", Key::Char('%')),
            ("+", Key::Char('+')),
            ("x", Key::Char('x')),
            ("ENTER", Key::Enter),
            ("PageUp", Key::PageUp),
            ("F5", Key::F(5)),
            ("é", Key::Char('é')),
        ] {
            assert_eq!(parse_chord(s), Some((k, nm)), "{s}");
        }
        assert_eq!(
            parse_chord("ctrl+]"),
            Some((Key::Char(']'), mods(true, false, false)))
        );
        assert_eq!(
            parse_chord("ctrl++"),
            Some((Key::Char('+'), mods(true, false, false)))
        );
        assert_eq!(
            parse_chord("Ctrl+Alt+Shift++"),
            Some((Key::Char('+'), mods(true, true, true)))
        );
        assert_eq!(
            parse_chord("CTRL+shift+T"),
            Some((Key::Char('t'), mods(true, false, true)))
        );
        assert_eq!(
            parse_chord("alt+F4"),
            Some((Key::F(4), mods(false, true, false)))
        );
        assert_eq!(
            parse_chord("alt+A"),
            Some((Key::Char('A'), mods(false, true, false)))
        );
        assert_eq!(
            parse_chord("ctrl+space"),
            Some((Key::Char(' '), mods(true, false, false)))
        );
    }

    #[test]
    fn chord_invalidos() {
        for s in [
            "",
            "++",
            "+++",
            "ctrl+",
            "ctrl",
            "+a",
            "ctrl+ctrl2+a",
            "meta+a",
            "f0",
            "f13",
            "f01",
            "ctrl+xx",
            "enter+",
            "ctrl+alt",
            "foo",
            "ctrl +a",
            "ctrl+f1+",
            "ctrl+,+a",
        ] {
            assert_eq!(parse_chord(s), None, "{s:?}");
        }
    }

    fn prof(e: Emulation) -> TerminalProfile {
        TerminalProfile {
            emulation: e,
            ..TerminalProfile::default()
        }
    }

    fn enc_e(e: Emulation, k: Key, m: Modifiers) -> Vec<u8> {
        Terminal::with_profile(5, 20, &prof(e)).encode_key(k, m)
    }

    #[test]
    fn term_names() {
        assert_eq!(Terminal::term_name(Emulation::Xterm), "xterm-256color");
        assert_eq!(Terminal::term_name(Emulation::Vt100), "vt100");
        assert_eq!(Terminal::term_name(Emulation::Vt220), "vt220");
        assert_eq!(Terminal::term_name(Emulation::Linux), "linux");
        assert_eq!(Terminal::term_name(Emulation::Ansi), "ansi");
    }

    #[test]
    fn new_equivale_al_perfil_por_defecto() {
        let a = Terminal::new(5, 20, 10_000);
        let b = Terminal::with_profile(5, 20, &TerminalProfile::default());
        assert!(a.profile_warnings().is_empty());
        for k in [
            Key::Up,
            Key::Home,
            Key::End,
            Key::Insert,
            Key::Delete,
            Key::PageUp,
            Key::F(1),
            Key::F(5),
            Key::F(12),
            Key::BackTab,
            Key::Backspace,
            Key::Char('a'),
        ] {
            for m in [none(), mods(true, false, false), mods(false, true, true)] {
                assert_eq!(a.encode_key(k, m), b.encode_key(k, m));
            }
        }
        assert_eq!(a.encode_key(Key::Backspace, none()), vec![0x7f]);
        assert_eq!(
            a.encode_key(Key::Up, mods(true, false, false)),
            b"\x1b[1;5A"
        );
        assert_eq!(a.encode_key(Key::F(3), none()), b"\x1bOR");
    }

    #[test]
    fn vt100_teclas() {
        let e = Emulation::Vt100;
        assert_eq!(enc_e(e, Key::F(1), none()), b"\x1bOP");
        assert_eq!(enc_e(e, Key::F(4), mods(true, false, false)), b"\x1bOS");
        for k in [
            Key::F(5),
            Key::F(12),
            Key::Home,
            Key::End,
            Key::Insert,
            Key::Delete,
            Key::PageUp,
            Key::PageDown,
            Key::BackTab,
        ] {
            assert!(enc_e(e, k, none()).is_empty(), "{k:?}");
        }
        assert_eq!(enc_e(e, Key::Up, mods(true, false, false)), b"\x1b[A");
        let mut t = Terminal::with_profile(5, 20, &prof(e));
        t.process(b"\x1b[?1h");
        assert_eq!(t.encode_key(Key::Up, none()), b"\x1bOA");
    }

    #[test]
    fn vt220_teclas() {
        let e = Emulation::Vt220;
        assert_eq!(enc_e(e, Key::F(1), none()), b"\x1bOP");
        assert_eq!(enc_e(e, Key::F(4), mods(false, false, true)), b"\x1bOS");
        assert_eq!(enc_e(e, Key::F(5), none()), b"\x1b[15~");
        assert_eq!(enc_e(e, Key::F(6), none()), b"\x1b[17~");
        assert_eq!(enc_e(e, Key::F(12), mods(true, false, false)), b"\x1b[24~");
        assert_eq!(enc_e(e, Key::Home, none()), b"\x1b[1~");
        assert_eq!(enc_e(e, Key::End, none()), b"\x1b[4~");
        assert_eq!(enc_e(e, Key::Delete, none()), b"\x1b[3~");
        assert_eq!(enc_e(e, Key::Right, mods(true, false, true)), b"\x1b[C");
        assert_eq!(enc_e(e, Key::BackTab, none()), b"\x1b[Z");
    }

    #[test]
    fn linux_teclas() {
        let e = Emulation::Linux;
        for (n, s) in [(1, "A"), (2, "B"), (3, "C"), (4, "D"), (5, "E")] {
            assert_eq!(
                enc_e(e, Key::F(n), none()),
                format!("\x1b[[{s}").into_bytes()
            );
        }
        assert_eq!(enc_e(e, Key::F(6), none()), b"\x1b[17~");
        assert_eq!(enc_e(e, Key::F(12), none()), b"\x1b[24~");
        assert_eq!(enc_e(e, Key::Home, none()), b"\x1b[1~");
        assert_eq!(enc_e(e, Key::End, none()), b"\x1b[4~");
        assert_eq!(enc_e(e, Key::Insert, none()), b"\x1b[2~");
        assert_eq!(enc_e(e, Key::PageDown, none()), b"\x1b[6~");
        assert_eq!(enc_e(e, Key::Up, mods(true, false, false)), b"\x1b[A");
    }

    #[test]
    fn ansi_teclas() {
        let e = Emulation::Ansi;
        assert_eq!(enc_e(e, Key::Up, mods(true, false, false)), b"\x1b[A");
        assert_eq!(enc_e(e, Key::Home, none()), b"\x1b[H");
        assert_eq!(enc_e(e, Key::Insert, none()), b"\x1b[L");
        assert_eq!(enc_e(e, Key::F(1), mods(false, false, true)), b"\x1bOP");
        assert_eq!(enc_e(e, Key::F(5), mods(true, false, false)), b"\x1b[15~");
        // Alt en caracteres sigue siendo prefijo ESC.
        assert_eq!(enc_e(e, Key::Char('x'), mods(false, true, false)), b"\x1bx");
    }

    #[test]
    fn xterm_conserva_modificadores() {
        let e = Emulation::Xterm;
        assert_eq!(enc_e(e, Key::Up, mods(false, true, false)), b"\x1b[1;3A");
        assert_eq!(enc_e(e, Key::F(1), mods(true, false, false)), b"\x1b[1;5P");
        assert_eq!(
            enc_e(e, Key::Delete, mods(true, false, false)),
            b"\x1b[3;5~"
        );
    }

    #[test]
    fn retroceso() {
        let p = TerminalProfile {
            backspace: BackspaceMode::CtrlH,
            ..TerminalProfile::default()
        };
        let t = Terminal::with_profile(5, 20, &p);
        assert_eq!(t.encode_key(Key::Backspace, none()), vec![0x08]);
        assert_eq!(
            t.encode_key(Key::Backspace, mods(false, true, false)),
            vec![0x1b, 0x08]
        );
        assert_eq!(enc_e(Emulation::Linux, Key::Backspace, none()), vec![0x7f]);
    }

    #[test]
    fn keymap_y_avisos() {
        let mut p = TerminalProfile::default();
        for (k, v) in [
            ("ctrl+a", "\\eOP"),
            ("F5", "A\\x42\\n\\t\\r\\\\"),
            ("ctrl+Shift+t", "é"),
            ("f13", "x"),
            ("f2", "\\q"),
            ("f3", "\\x4"),
            ("f4", "abc\\"),
            ("nope", "x"),
        ] {
            p.keymap.insert(k.into(), v.into());
        }
        let t = Terminal::with_profile(5, 20, &p);
        assert_eq!(t.profile_warnings().len(), 5, "{:?}", t.profile_warnings());
        // El keymap gana al comportamiento normal (ctrl+a sería 0x01).
        assert_eq!(
            t.encode_key(Key::Char('a'), mods(true, false, false)),
            b"\x1bOP"
        );
        assert_eq!(t.encode_key(Key::F(5), none()), b"AB\n\t\r\\");
        assert_eq!(
            t.encode_key(Key::Char('t'), mods(true, false, true)),
            "é".as_bytes()
        );
        // Sin entrada válida, lo de siempre.
        assert_eq!(t.encode_key(Key::F(2), none()), b"\x1bOQ");
        assert_eq!(t.encode_key(Key::Char('a'), none()), b"a");
    }

    #[test]
    fn scrollback_del_perfil() {
        let mut p = TerminalProfile {
            scrollback: 3,
            ..TerminalProfile::default()
        };
        let mut t = Terminal::with_profile(2, 10, &p);
        t.process(b"1\r\n2\r\n3\r\n4\r\n5\r\n6\r\n7\r\n8");
        assert_eq!(t.scrollback_len(), 3);
        p.scrollback = 0;
        let mut t = Terminal::with_profile(2, 10, &p);
        t.process(b"1\r\n2\r\n3\r\n4");
        assert_eq!(t.scrollback_len(), 0);
    }

    /// 3 filas x 10 columnas, líneas l0..l9: scrollback 7, pantalla l7..l9.
    fn diez() -> Terminal {
        let mut t = Terminal::new(3, 10, 100);
        let s: Vec<String> = (0..10).map(|i| format!("l{i}")).collect();
        t.process(s.join("\r\n").as_bytes());
        t
    }

    fn m(row: usize, start_col: u16, end_col: u16) -> Match {
        Match {
            row,
            start_col,
            end_col,
        }
    }

    #[test]
    fn busca_en_scrollback_y_pantalla() {
        let mut t = diez();
        assert_eq!(t.scrollback_len(), 7);
        assert_eq!(t.search("l0", true), vec![m(0, 0, 2)]);
        assert_eq!(t.search("l9", true), vec![m(9, 0, 2)]);
        assert_eq!(t.search("l", true).len(), 10);
        let rows: Vec<usize> = t.search("l", true).iter().map(|x| x.row).collect();
        assert_eq!(rows, (0..10).collect::<Vec<_>>());
        // No altera la vista.
        t.scroll_view(4);
        let (off, top) = (t.scroll_offset(), t.top_row());
        let before = t.screen();
        assert_eq!(t.search("l2", true), vec![m(2, 0, 2)]);
        assert_eq!((t.scroll_offset(), t.top_row()), (off, top));
        assert_eq!(t.screen(), before);
    }

    #[test]
    fn busca_mayusculas_y_varias_por_linea() {
        let mut t = Terminal::new(3, 30, 10);
        t.process(b"Hello hello HELLO");
        let all = t.search("hello", false);
        assert_eq!(all, vec![m(0, 0, 5), m(0, 6, 11), m(0, 12, 17)]);
        assert_eq!(t.search("hello", true), vec![m(0, 6, 11)]);
        assert_eq!(t.search("HELLO", true), vec![m(0, 12, 17)]);
        assert_eq!(t.search("HeLLo", false).len(), 3);
        // No solapadas.
        let mut t = Terminal::new(2, 10, 0);
        t.process(b"aaaa");
        assert_eq!(t.search("aa", true), vec![m(0, 0, 2), m(0, 2, 4)]);
        // Espacios internos sí; los de relleno al final no.
        t.process(b"\r\nx y");
        assert_eq!(t.search("x y", true), vec![m(1, 0, 3)]);
        assert!(t.search("y ", true).is_empty());
    }

    #[test]
    fn busca_con_anchos() {
        let mut t = Terminal::new(2, 20, 0);
        t.process("ab中文cd".as_bytes());
        assert_eq!(t.search("中", true), vec![m(0, 2, 4)]);
        assert_eq!(t.search("文c", true), vec![m(0, 4, 7)]);
        assert_eq!(t.search("b中", true), vec![m(0, 1, 4)]);
        assert_eq!(t.search("cd", true), vec![m(0, 6, 8)]);
        assert_eq!(t.search("中文", true), vec![m(0, 2, 6)]);
        let mut t = Terminal::new(2, 20, 0);
        t.process("x😀y😀".as_bytes());
        assert_eq!(t.search("y", true), vec![m(0, 3, 4)]);
        assert_eq!(t.search("😀", true), vec![m(0, 1, 3), m(0, 4, 6)]);
        // Minúsculas Unicode.
        let mut t = Terminal::new(2, 20, 0);
        t.process("ÁRBOL árbol".as_bytes());
        assert_eq!(t.search("árbol", false).len(), 2);
        assert_eq!(t.search("árbol", true), vec![m(0, 6, 11)]);
    }

    #[test]
    fn busca_bordes() {
        let t = Terminal::new(3, 10, 10);
        assert!(t.search("x", true).is_empty());
        assert!(t.search("", true).is_empty());
        assert!(t.search(" ", false).is_empty());
        // Scrollback 0: solo la pantalla, filas 0..rows.
        let mut t = Terminal::new(3, 10, 0);
        t.process(b"l0\r\nl1\r\nl2\r\nl3\r\nl4");
        assert_eq!(t.scrollback_len(), 0);
        assert!(t.search("l0", true).is_empty());
        assert_eq!(t.search("l2", true), vec![m(0, 0, 2)]);
        assert_eq!(t.search("l4", true), vec![m(2, 0, 2)]);
        // Una línea partida por ancho no casa a través del corte (límite documentado).
        let mut t = Terminal::new(3, 5, 0);
        t.process(b"abcdefgh");
        assert!(t.search("efg", true).is_empty());
        assert_eq!(t.search("fgh", true), vec![m(1, 0, 3)]);
    }

    #[test]
    fn busca_tras_redimensionar() {
        let mut t = diez();
        t.resize(3, 5);
        for i in 0..10 {
            let r = t.search(&format!("l{i}"), true);
            assert_eq!(r.len(), 1, "l{i}: {r:?}");
        }
        t.resize(6, 20);
        let total = t.scrollback_len() + 6;
        for i in 0..10 {
            let r = t.search(&format!("l{i}"), true);
            assert_eq!(r.len(), 1, "l{i}: {r:?}");
            assert!(r[0].row < total);
        }
        t.resize(1, 20);
        assert_eq!(t.search("l9", true).len(), 1);
        assert!(
            t.text_range((0, 0), (usize::MAX, u16::MAX), false)
                .contains("l9")
        );
    }

    #[test]
    fn reveal_y_top() {
        let mut t = diez();
        assert_eq!(t.top_row(), 7);
        t.reveal_row(0);
        assert_eq!((t.top_row(), t.scroll_offset()), (0, 7));
        t.reveal_row(4);
        assert_eq!(t.top_row(), 3); // centrada: 4 - 3/2
        t.reveal_row(2);
        assert_eq!(t.top_row(), 1);
        assert_eq!(t.screen().cells[0].text, "l");
        assert_eq!(t.screen().cells[1].text, "1");
        t.reveal_row(9);
        assert_eq!((t.top_row(), t.scroll_offset()), (7, 0));
        t.reveal_row(1000);
        assert_eq!(t.top_row(), 7);
        t.reveal_row(8);
        assert_eq!(t.top_row(), 7);
        t.scroll_view(2);
        assert_eq!(t.top_row(), 5);
    }

    #[test]
    fn reveal_en_pantalla_alternativa_no_hace_nada() {
        let mut t = diez();
        t.process(b"\x1b[?1049h");
        assert!(t.alternate_screen());
        assert_eq!(t.top_row(), 0);
        t.process(b"alt");
        t.reveal_row(5);
        assert_eq!((t.top_row(), t.scroll_offset()), (0, 0));
        // Sin scrollback: solo se busca en la pantalla alternativa.
        assert_eq!(t.search("alt", true), vec![m(0, 0, 3)]);
        assert!(t.search("l0", true).is_empty());
        t.process(b"\x1b[?1049l");
        assert_eq!(t.search("l0", true), vec![m(0, 0, 2)]);
    }

    #[test]
    fn reveal_sin_scrollback() {
        let mut t = Terminal::new(3, 10, 0);
        t.process(b"a");
        t.reveal_row(2);
        assert_eq!((t.top_row(), t.scroll_offset()), (0, 0));
    }

    #[test]
    fn texto_basico() {
        let mut t = Terminal::new(4, 10, 10);
        t.process(b"foo   \r\nbar\r\n\r\nbaz");
        assert_eq!(t.text_range((0, 0), (1, 2), false), "foo\nbar");
        assert_eq!(t.text_range((1, 2), (0, 0), false), "foo\nbar");
        assert_eq!(t.text_range((0, 1), (0, 1), false), "o");
        assert_eq!(t.text_range((0, 1), (3, 1), false), "oo\nbar\n\nba");
        assert_eq!(t.text_range((0, 0), (99, 99), false), "foo\nbar\n\nbaz");
        assert_eq!(t.text_range((0, 4), (0, 9), false), "");
        assert_eq!(t.text_range((2, 0), (2, 5), false), "");
    }

    #[test]
    fn texto_bloque() {
        let mut t = Terminal::new(3, 10, 0);
        t.process(b"abcd\r\nefgh\r\nijkl");
        assert_eq!(t.text_range((0, 1), (2, 2), true), "bc\nfg\njk");
        assert_eq!(t.text_range((2, 2), (0, 1), true), "bc\nfg\njk");
        assert_eq!(t.text_range((2, 1), (0, 2), true), "bc\nfg\njk");
        assert_eq!(t.text_range((0, 3), (1, 6), true), "d\nh");
        assert_eq!(t.text_range((1, 2), (1, 2), true), "g");
        assert_eq!(t.text_range((0, 0), (2, 99), true), "abcd\nefgh\nijkl");
    }

    #[test]
    fn texto_con_wrap_y_scrollback() {
        let mut t = Terminal::new(2, 5, 10);
        t.process(b"abcdefgh\r\nxy\r\nlast");
        // abcde (wrap) fgh / xy / last: scrollback 2 filas.
        assert_eq!(t.scrollback_len(), 2);
        assert_eq!(t.text_range((0, 0), (3, 3), false), "abcdefgh\nxy\nlast");
        assert_eq!(t.text_range((0, 3), (1, 1), false), "defg");
        assert_eq!(t.text_range((1, 0), (2, 1), false), "fgh\nxy");
        // El bloque no une líneas partidas.
        assert_eq!(t.text_range((0, 0), (1, 1), true), "ab\nfg");
        // Una vista desplazada no cambia el resultado.
        t.scroll_view(2);
        assert_eq!(t.text_range((0, 0), (3, 3), false), "abcdefgh\nxy\nlast");
        assert_eq!(t.scroll_offset(), 2);
    }

    #[test]
    fn texto_con_anchos() {
        let mut t = Terminal::new(2, 10, 0);
        t.process("中文a😀".as_bytes());
        // cols: 中 0-1, 文 2-3, a 4, 😀 5-6
        assert_eq!(t.text_range((0, 0), (0, 0), false), "中");
        assert_eq!(t.text_range((0, 1), (0, 1), false), "中");
        assert_eq!(t.text_range((0, 1), (0, 2), false), "中文");
        assert_eq!(t.text_range((0, 3), (0, 4), false), "文a");
        assert_eq!(t.text_range((0, 0), (0, 9), false), "中文a😀");
        assert_eq!(t.text_range((0, 6), (0, 6), false), "😀");
        assert_eq!(t.text_range((0, 3), (0, 3), true), "文");
        assert_eq!(t.text_range((0, 1), (0, 4), true), "中文a");
    }

    #[test]
    fn texto_buffer_vacio() {
        let t = Terminal::new(2, 5, 0);
        assert_eq!(t.text_range((0, 0), (0, 4), false), "");
        assert_eq!(t.text_range((0, 0), (1, 4), false), "\n");
        assert_eq!(t.text_range((0, 0), (0, 4), true), "");
    }

    /// Rendimiento: `cargo test -p friki-term --release -- --ignored --nocapture busca_rapido`.
    /// Medido en release (Apple Silicon, 10 040 filas x 120 cols, sin distinguir mayúsculas,
    /// 10 039 coincidencias, incluye clonar la pantalla): ~26 ms. Umbral: 50 ms.
    #[test]
    #[ignore]
    #[cfg(not(debug_assertions))] // solo tiene sentido medido en release
    fn busca_rapido() {
        let mut t = Terminal::new(40, 120, 10_000);
        let mut s = String::new();
        for i in 0..10_040 {
            s.push_str(&format!(
                "{i:05} línea de prueba con texto largo para rellenar la fila ok\r\n"
            ));
        }
        t.process(s.as_bytes());
        assert_eq!(t.scrollback_len(), 10_000);
        let t0 = std::time::Instant::now();
        let r = t.search("rellenar", false);
        let dt = t0.elapsed();
        eprintln!(
            "buscar en {} filas: {dt:?} ({} coincidencias)",
            t.scrollback_len() + 40,
            r.len()
        );
        assert!(r.len() >= 10_000);
        assert!(dt < std::time::Duration::from_millis(50), "{dt:?}");
    }
}
