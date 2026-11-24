//! Lógica pura de entrada: traducción crossterm -> friki_term y prefijo Ctrl+].

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use friki_term::{Key, Modifiers};

/// Traduce una tecla de crossterm. `None` para teclas sin equivalente o de tipo "release".
pub fn translate_key(ev: &KeyEvent) -> Option<(Key, Modifiers)> {
    if ev.kind == KeyEventKind::Release {
        return None;
    }
    let mut mods = Modifiers {
        ctrl: ev.modifiers.contains(KeyModifiers::CONTROL),
        alt: ev.modifiers.contains(KeyModifiers::ALT),
        shift: ev.modifiers.contains(KeyModifiers::SHIFT),
    };
    let key = match ev.code {
        KeyCode::Char(c) => Key::Char(c),
        KeyCode::Enter => Key::Enter,
        KeyCode::Backspace => Key::Backspace,
        KeyCode::Tab => Key::Tab,
        KeyCode::BackTab => {
            mods.shift = true;
            Key::BackTab
        }
        KeyCode::Esc => Key::Esc,
        KeyCode::Up => Key::Up,
        KeyCode::Down => Key::Down,
        KeyCode::Left => Key::Left,
        KeyCode::Right => Key::Right,
        KeyCode::Home => Key::Home,
        KeyCode::End => Key::End,
        KeyCode::PageUp => Key::PageUp,
        KeyCode::PageDown => Key::PageDown,
        KeyCode::Insert => Key::Insert,
        KeyCode::Delete => Key::Delete,
        KeyCode::F(n) => Key::F(n),
        _ => return None,
    };
    Some((key, mods))
}

/// Resultado de pasar una tecla por la máquina de estados del prefijo.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrefixOut {
    /// Tecla normal: enviar al remoto.
    Forward,
    /// Se pulsó Ctrl+]: esperar la siguiente tecla.
    Armed,
    /// Ctrl+] Ctrl+]: enviar un Ctrl+] literal al remoto.
    Literal,
    Quit,
    ScrollUp,
    ScrollDown,
    Help,
    /// Tecla desconocida tras el prefijo: se descarta.
    Cancel,
}

#[derive(Debug, Default)]
pub struct Prefix {
    armed: bool,
}

/// Ctrl+] llega como `Char(']')` (kitty) o como `Char('5')` (terminales clásicos, byte 0x1d).
fn is_prefix(ev: &KeyEvent) -> bool {
    ev.modifiers.contains(KeyModifiers::CONTROL) && matches!(ev.code, KeyCode::Char(']' | '5'))
}

impl Prefix {
    pub fn feed(&mut self, ev: &KeyEvent) -> PrefixOut {
        if ev.kind == KeyEventKind::Release {
            return PrefixOut::Cancel;
        }
        if !self.armed {
            if is_prefix(ev) {
                self.armed = true;
                return PrefixOut::Armed;
            }
            return PrefixOut::Forward;
        }
        self.armed = false;
        if is_prefix(ev) {
            return PrefixOut::Literal;
        }
        match ev.code {
            KeyCode::Char('q' | 'Q') => PrefixOut::Quit,
            KeyCode::PageUp => PrefixOut::ScrollUp,
            KeyCode::PageDown => PrefixOut::ScrollDown,
            KeyCode::Char('?') => PrefixOut::Help,
            _ => PrefixOut::Cancel,
        }
    }

    pub fn is_armed(&self) -> bool {
        self.armed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn k(code: KeyCode, m: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, m)
    }

    #[test]
    fn translates_modifiers_and_keys() {
        let (key, mods) =
            translate_key(&k(KeyCode::Char('c'), KeyModifiers::CONTROL)).expect("tecla");
        assert_eq!(key, Key::Char('c'));
        assert!(mods.ctrl && !mods.alt && !mods.shift);
        let (key, mods) = translate_key(&k(KeyCode::BackTab, KeyModifiers::NONE)).expect("tecla");
        assert_eq!((key, mods.shift), (Key::BackTab, true));
        assert_eq!(
            translate_key(&k(KeyCode::F(5), KeyModifiers::NONE)).map(|t| t.0),
            Some(Key::F(5))
        );
        assert!(translate_key(&k(KeyCode::CapsLock, KeyModifiers::NONE)).is_none());
        let mut rel = k(KeyCode::Char('a'), KeyModifiers::NONE);
        rel.kind = KeyEventKind::Release;
        assert!(translate_key(&rel).is_none());
    }

    #[test]
    fn prefix_state_machine() {
        let mut p = Prefix::default();
        let a = k(KeyCode::Char('a'), KeyModifiers::NONE);
        let q = k(KeyCode::Char('q'), KeyModifiers::NONE);
        let pre = k(KeyCode::Char(']'), KeyModifiers::CONTROL);
        assert_eq!(p.feed(&a), PrefixOut::Forward);
        // Sin prefijo, 'q' va al remoto.
        assert_eq!(p.feed(&q), PrefixOut::Forward);
        assert_eq!(p.feed(&pre), PrefixOut::Armed);
        assert!(p.is_armed());
        assert_eq!(p.feed(&q), PrefixOut::Quit);
        assert!(!p.is_armed());
        // Variante clásica Ctrl+5 (0x1d).
        assert_eq!(
            p.feed(&k(KeyCode::Char('5'), KeyModifiers::CONTROL)),
            PrefixOut::Armed
        );
        assert_eq!(
            p.feed(&k(KeyCode::PageUp, KeyModifiers::NONE)),
            PrefixOut::ScrollUp
        );
        p.feed(&pre);
        assert_eq!(
            p.feed(&k(KeyCode::PageDown, KeyModifiers::NONE)),
            PrefixOut::ScrollDown
        );
        p.feed(&pre);
        assert_eq!(
            p.feed(&k(KeyCode::Char('?'), KeyModifiers::NONE)),
            PrefixOut::Help
        );
        p.feed(&pre);
        assert_eq!(p.feed(&pre), PrefixOut::Literal);
        p.feed(&pre);
        assert_eq!(p.feed(&a), PrefixOut::Cancel);
        assert_eq!(p.feed(&a), PrefixOut::Forward);
    }
}
