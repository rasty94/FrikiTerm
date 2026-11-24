//! Atajos configurables: prefijo + tecla -> acción, con la semántica de `AppConfig`.
//! Lógica pura de entrada, sin E/S.

use crossterm::event::{KeyEvent, KeyEventKind};
use friki_core::config::AppConfig;
use friki_term::{Key, Modifiers};

use crate::input::{pressed, translate_key};

type Chord = (Key, Modifiers);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    NewTab,
    ClosePane,
    NextTab,
    PrevTab,
    SplitHorizontal,
    SplitVertical,
    FocusNext,
    FocusPrev,
    ScrollUp,
    ScrollDown,
    Sessions,
    RenameTab,
    Search,
    CopyMode,
    Help,
    Quit,
}

/// (nombre en `AppConfig.bindings`, acción, descripción para la ayuda).
const ACTIONS: [(&str, Action, &str); 16] = [
    ("new-tab", Action::NewTab, "pestaña nueva"),
    ("close-pane", Action::ClosePane, "cerrar el panel"),
    ("next-tab", Action::NextTab, "pestaña siguiente"),
    ("prev-tab", Action::PrevTab, "pestaña anterior"),
    (
        "split-horizontal",
        Action::SplitHorizontal,
        "dividir arriba/abajo",
    ),
    (
        "split-vertical",
        Action::SplitVertical,
        "dividir izquierda/derecha",
    ),
    ("focus-next", Action::FocusNext, "panel siguiente"),
    ("focus-prev", Action::FocusPrev, "panel anterior"),
    ("scroll-up", Action::ScrollUp, "scrollback arriba"),
    ("scroll-down", Action::ScrollDown, "scrollback abajo"),
    ("sessions", Action::Sessions, "abrir sesión (pestaña nueva)"),
    ("rename-tab", Action::RenameTab, "renombrar la pestaña"),
    ("search", Action::Search, "buscar (pendiente)"),
    ("copy-mode", Action::CopyMode, "modo copia (pendiente)"),
    ("help", Action::Help, "esta ayuda"),
    ("quit", Action::Quit, "salir"),
];

fn ctrl() -> Modifiers {
    Modifiers {
        ctrl: true,
        ..Default::default()
    }
}

/// Interpreta `"ctrl+]"`, `"shift+o"`, `"pageup"`, `"f5"`... Misma semántica que
/// `friki_term::parse_chord`: modificadores `ctrl`/`alt`/`shift` separados por `+` y una
/// tecla final (un carácter, un nombre o `f1`..`f12`), sin distinguir mayúsculas en
/// nombres y modificadores.
// ponytail: copia local porque `friki_term::parse_chord` aún es `todo!()` en esta rama;
// sustituir por la de friki-term y borrar esta.
pub fn parse_chord_local(s: &str) -> Option<Chord> {
    let (mods_part, key_part, had_sep) = if s == "+" {
        ("", "+", false)
    } else if let Some(m) = s.strip_suffix("++") {
        (m, "+", true)
    } else if let Some((m, k)) = s.rsplit_once('+') {
        (m, k, true)
    } else {
        ("", s, false)
    };
    if had_sep && mods_part.is_empty() {
        return None;
    }
    let mut mods = Modifiers::default();
    if !mods_part.is_empty() {
        for m in mods_part.split('+') {
            match m.to_lowercase().as_str() {
                "ctrl" => mods.ctrl = true,
                "alt" => mods.alt = true,
                "shift" => mods.shift = true,
                _ => return None,
            }
        }
    }
    let mut chars = key_part.chars();
    let key = match (chars.next(), chars.next()) {
        (None, _) => return None,
        (Some(c), None) => Key::Char(c),
        _ => match key_part.to_lowercase().as_str() {
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
            f => {
                let n: u8 = f.strip_prefix('f')?.parse().ok()?;
                if !(1..=12).contains(&n) {
                    return None;
                }
                Key::F(n)
            }
        },
    };
    Some((key, mods))
}

/// Forma canónica para comparar: letras en minúscula con `shift` si eran mayúsculas;
/// símbolos sin `shift` (los terminales no lo marcan igual para `"` o `%`); `Ctrl+5`
/// (byte 0x1d en terminales clásicos) es `Ctrl+]`.
fn canon((key, m): Chord) -> Chord {
    match key {
        Key::Char(c) if c.is_alphabetic() => (
            Key::Char(c.to_lowercase().next().unwrap_or(c)),
            Modifiers {
                shift: m.shift || c.is_uppercase(),
                ..m
            },
        ),
        Key::Char('5') if m.ctrl => (Key::Char(']'), m),
        Key::Char(_) => (key, Modifiers { shift: false, ..m }),
        Key::BackTab => (key, Modifiers { shift: true, ..m }),
        _ => (key, m),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyOut {
    /// Tecla normal: enviar al remoto del panel con foco.
    Forward(Key, Modifiers),
    /// Se pulsó el prefijo: esperar la siguiente tecla.
    Armed,
    /// Prefijo dos veces: enviar el prefijo al remoto.
    Literal(Key, Modifiers),
    Do(Action),
    /// Tecla desconocida tras el prefijo: se descarta.
    Cancel,
    /// Soltar/repetir el prefijo o teclas sin equivalente: nada.
    Ignore,
}

pub struct Keymap {
    /// Tal como está en la configuración (es lo que se envía con prefijo doble).
    prefix_raw: Chord,
    prefix: Chord,
    prefix_text: String,
    binds: Vec<(Chord, Action, String)>,
    armed: bool,
}

impl Keymap {
    /// Un prefijo no válido cae al de `AppConfig::default()`; un atajo no válido o una
    /// acción desconocida se ignoran.
    pub fn new(cfg: &AppConfig) -> Self {
        let default = AppConfig::default();
        let (raw, prefix_text) = match parse_chord_local(&cfg.prefix) {
            Some(c) => (c, cfg.prefix.clone()),
            None => (
                parse_chord_local(&default.prefix).unwrap_or((Key::Char(']'), ctrl())),
                default.prefix.clone(),
            ),
        };
        let binds = ACTIONS
            .iter()
            .filter_map(|(name, action, _)| {
                let text = cfg.bindings.get(*name)?;
                Some((canon(parse_chord_local(text)?), *action, text.clone()))
            })
            .collect();
        Self {
            prefix_raw: raw,
            prefix: canon(raw),
            prefix_text,
            binds,
            armed: false,
        }
    }

    pub fn is_armed(&self) -> bool {
        self.armed
    }

    /// Procesa una tecla por la máquina de estados del prefijo.
    pub fn feed(&mut self, ev: &KeyEvent) -> KeyOut {
        if !pressed(ev) {
            return KeyOut::Ignore;
        }
        let Some(raw) = translate_key(ev) else {
            return if std::mem::take(&mut self.armed) {
                KeyOut::Cancel
            } else {
                KeyOut::Ignore
            };
        };
        let c = canon(raw);
        let is_prefix = c == self.prefix;
        if ev.kind == KeyEventKind::Repeat && is_prefix {
            return KeyOut::Ignore; // repetir el prefijo no cambia el estado
        }
        if !self.armed {
            if is_prefix {
                self.armed = true;
                return KeyOut::Armed;
            }
            return KeyOut::Forward(raw.0, raw.1);
        }
        self.armed = false;
        if is_prefix {
            return KeyOut::Literal(self.prefix_raw.0, self.prefix_raw.1);
        }
        self.binds
            .iter()
            .find(|(b, ..)| *b == c)
            .map_or(KeyOut::Cancel, |(_, a, _)| KeyOut::Do(*a))
    }

    /// Texto del atajo de una acción (vacío si no está asignada).
    pub fn key_for(&self, action: Action) -> &str {
        self.binds
            .iter()
            .find(|(_, a, _)| *a == action)
            .map_or("", |(_, _, t)| t.as_str())
    }

    /// Resumen para la barra de estado.
    pub fn hint(&self) -> String {
        format!(
            " {p} {q} salir  {h} ayuda ",
            p = self.prefix_text,
            q = self.key_for(Action::Quit),
            h = self.key_for(Action::Help)
        )
    }

    pub fn help_lines(&self) -> Vec<String> {
        let p = &self.prefix_text;
        let mut v = vec![format!("Atajos (prefijo {p}, como tmux):")];
        for (_, action, desc) in ACTIONS {
            let k = self.key_for(action);
            if !k.is_empty() {
                v.push(format!("  {p} {k:<10} {desc}"));
            }
        }
        v.push(format!("  {p} {p}   enviar el prefijo al remoto"));
        v.push("(cualquier tecla cierra la ayuda)".into());
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyModifiers};

    fn k(code: KeyCode, m: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, m)
    }
    fn plain(c: char) -> KeyEvent {
        k(KeyCode::Char(c), KeyModifiers::NONE)
    }
    fn pre() -> KeyEvent {
        k(KeyCode::Char(']'), KeyModifiers::CONTROL)
    }

    #[test]
    fn parses_chords() {
        let m = |ctrl, alt, shift| Modifiers { ctrl, alt, shift };
        let none = Modifiers::default();
        assert_eq!(
            parse_chord_local("ctrl+]"),
            Some((Key::Char(']'), m(true, false, false)))
        );
        assert_eq!(
            parse_chord_local("Ctrl+Alt+F5"),
            Some((Key::F(5), m(true, true, false)))
        );
        assert_eq!(
            parse_chord_local("shift+o"),
            Some((Key::Char('o'), m(false, false, true)))
        );
        assert_eq!(
            parse_chord_local("ctrl++"),
            Some((Key::Char('+'), m(true, false, false)))
        );
        assert_eq!(parse_chord_local("+"), Some((Key::Char('+'), none)));
        assert_eq!(parse_chord_local("\""), Some((Key::Char('"'), none)));
        assert_eq!(parse_chord_local("PageUp"), Some((Key::PageUp, none)));
        assert_eq!(parse_chord_local("space"), Some((Key::Char(' '), none)));
        assert_eq!(parse_chord_local("f12").map(|c| c.0), Some(Key::F(12)));
        for bad in [
            "", "f13", "f0", "foo", "a+", "+x", "meta+a", "ctrl+", "ctrl+ab",
        ] {
            assert_eq!(parse_chord_local(bad), None, "{bad}");
        }
    }

    fn map() -> Keymap {
        Keymap::new(&AppConfig::default())
    }

    #[test]
    fn routes_default_bindings() {
        let mut km = map();
        assert_eq!(
            km.feed(&plain('a')),
            KeyOut::Forward(Key::Char('a'), Modifiers::default())
        );
        for (ev, act) in [
            (plain('c'), Action::NewTab),
            (plain('x'), Action::ClosePane),
            (plain('n'), Action::NextTab),
            (plain('p'), Action::PrevTab),
            (
                k(KeyCode::Char('"'), KeyModifiers::SHIFT),
                Action::SplitHorizontal,
            ),
            (plain('%'), Action::SplitVertical),
            (plain('o'), Action::FocusNext),
            (
                k(KeyCode::Char('O'), KeyModifiers::SHIFT),
                Action::FocusPrev,
            ),
            (plain('O'), Action::FocusPrev),
            (k(KeyCode::PageUp, KeyModifiers::NONE), Action::ScrollUp),
            (k(KeyCode::PageDown, KeyModifiers::NONE), Action::ScrollDown),
            (plain('s'), Action::Sessions),
            (plain(','), Action::RenameTab),
            (plain('/'), Action::Search),
            (plain('['), Action::CopyMode),
            (plain('?'), Action::Help),
            (plain('q'), Action::Quit),
        ] {
            assert_eq!(km.feed(&pre()), KeyOut::Armed);
            assert!(km.is_armed());
            assert_eq!(km.feed(&ev), KeyOut::Do(act), "{ev:?}");
            assert!(!km.is_armed());
        }
    }

    #[test]
    fn prefix_literal_cancel_and_repeat() {
        let mut km = map();
        // Ctrl+5 (0x1d) es el mismo prefijo en terminales clásicos.
        let c5 = k(KeyCode::Char('5'), KeyModifiers::CONTROL);
        assert_eq!(km.feed(&c5), KeyOut::Armed);
        let mut rep = pre();
        rep.kind = KeyEventKind::Repeat;
        assert_eq!(km.feed(&rep), KeyOut::Ignore);
        assert!(km.is_armed());
        let mut rel = plain('q');
        rel.kind = KeyEventKind::Release;
        assert_eq!(km.feed(&rel), KeyOut::Ignore);
        assert!(km.is_armed());
        assert_eq!(km.feed(&pre()), KeyOut::Literal(Key::Char(']'), ctrl()));
        assert!(!km.is_armed());
        km.feed(&pre());
        assert_eq!(km.feed(&plain('z')), KeyOut::Cancel);
        assert_eq!(
            km.feed(&plain('q')),
            KeyOut::Forward(Key::Char('q'), Modifiers::default())
        );
    }

    #[test]
    fn custom_prefix_and_invalid_fallback() {
        let mut cfg = AppConfig {
            prefix: "ctrl+b".into(),
            ..Default::default()
        };
        cfg.bindings.insert("quit".into(), "Q".into());
        cfg.bindings.insert("help".into(), "no-valida".into());
        let mut km = Keymap::new(&cfg);
        assert_eq!(km.feed(&pre()), KeyOut::Forward(Key::Char(']'), ctrl()));
        assert_eq!(
            km.feed(&k(KeyCode::Char('b'), KeyModifiers::CONTROL)),
            KeyOut::Armed
        );
        assert_eq!(km.feed(&plain('Q')), KeyOut::Do(Action::Quit));
        assert_eq!(km.key_for(Action::Help), "");
        cfg.prefix = "???".into();
        assert!(Keymap::new(&cfg).hint().starts_with(" ctrl+] "));
    }

    #[test]
    fn help_lists_every_bound_action() {
        let h = map().help_lines().join("\n");
        assert!(h.contains("ctrl+] q") && h.contains("ctrl+] %") && h.contains("ctrl+] ?"));
    }
}
