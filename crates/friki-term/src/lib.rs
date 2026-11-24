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

use friki_core::config::LogConfig;

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

pub struct Terminal {
    parser: vt100::Parser<TitleCb>,
}

impl Terminal {
    pub fn new(rows: u16, cols: u16, scrollback: usize) -> Self {
        Self {
            parser: vt100::Parser::new_with_callbacks(
                nz(rows),
                nz(cols),
                scrollback,
                TitleCb::default(),
            ),
        }
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
        let app = self.parser.screen().application_cursor();
        let m = 1 + u8::from(mods.shift) + 2 * u8::from(mods.alt) + 4 * u8::from(mods.ctrl);
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
            Key::Backspace => return alt(vec![0x7f]),
            Key::Tab => return alt(vec![b'\t']),
            Key::Esc => return alt(vec![0x1b]),
            _ => {}
        }
        match key {
            Key::BackTab => "\x1b[Z".into(),
            Key::Up => cursor('A'),
            Key::Down => cursor('B'),
            Key::Right => cursor('C'),
            Key::Left => cursor('D'),
            Key::Home => cursor('H'),
            Key::End => cursor('F'),
            Key::Insert => tilde("2"),
            Key::Delete => tilde("3"),
            Key::PageUp => tilde("5"),
            Key::PageDown => tilde("6"),
            Key::F(n @ 1..=4) => {
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
}
