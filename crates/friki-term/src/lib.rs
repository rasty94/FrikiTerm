//! Emulación de terminal sobre un crate existente (`vt100`, `alacritty_terminal`
//! o `termwiz`) y registro de sesión con enmascarado de contraseñas.
//! No conoce SSH ni la interfaz: entra `&[u8]`, sale `Screen`.

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

pub struct Terminal {
    _private: (),
}

impl Terminal {
    pub fn new(rows: u16, cols: u16, scrollback: usize) -> Self {
        let _ = (rows, cols, scrollback);
        todo!("friki-term: Terminal::new")
    }

    pub fn process(&mut self, bytes: &[u8]) {
        let _ = bytes;
        todo!("friki-term: Terminal::process")
    }

    pub fn resize(&mut self, rows: u16, cols: u16) {
        let _ = (rows, cols);
        todo!("friki-term: Terminal::resize")
    }

    /// Desplaza la vista del scrollback (positivo = hacia arriba). 0 vuelve al final.
    pub fn scroll_view(&mut self, lines: i32) {
        let _ = lines;
        todo!("friki-term: Terminal::scroll_view")
    }

    pub fn screen(&self) -> Screen {
        todo!("friki-term: Terminal::screen")
    }

    /// Respeta el modo cursor de aplicación y el keypad.
    pub fn encode_key(&self, key: Key, mods: Modifiers) -> Vec<u8> {
        let _ = (key, mods);
        todo!("friki-term: Terminal::encode_key")
    }

    /// Usa bracketed paste si el remoto lo activó.
    pub fn encode_paste(&self, text: &str) -> Vec<u8> {
        let _ = text;
        todo!("friki-term: Terminal::encode_paste")
    }
}

/// Registro de sesión. La salida se registra siempre; la entrada solo si
/// `log_input`, y nunca la línea que sigue a un aviso de contraseña o
/// passphrase visto en la salida (se escribe `[entrada oculta]`).
pub struct SessionLogger {
    _private: (),
}

impl SessionLogger {
    /// Expande `%S`, `%H` y `%D` en `cfg.path`, relativo a `base_dir`.
    pub fn open(cfg: &LogConfig, base_dir: &Path, session: &str, host: &str) -> std::io::Result<Self> {
        let _ = (cfg, base_dir, session, host);
        todo!("friki-term: SessionLogger::open")
    }

    pub fn log_output(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        let _ = bytes;
        todo!("friki-term: SessionLogger::log_output")
    }

    pub fn log_input(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        let _ = bytes;
        todo!("friki-term: SessionLogger::log_input")
    }
}
