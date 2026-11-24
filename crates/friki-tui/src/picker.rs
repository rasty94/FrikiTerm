//! Selector de sesión: lista de sesiones guardadas filtrable escribiendo, o un destino
//! `[user@]host[:port]`. Estado puro, sin E/S.

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use friki_core::config::{SessionConfig, resolve_target};

const ROWS: usize = 8;

#[derive(Debug, PartialEq)]
pub enum PickerOut {
    Pending,
    Cancel,
    Open(Box<SessionConfig>),
}

pub struct Picker {
    sessions: Vec<SessionConfig>,
    pub filter: String,
    sel: usize,
    pub error: Option<String>,
}

fn matches(s: &SessionConfig, f: &str) -> bool {
    let f = f.to_lowercase();
    let user_host = format!("{}@{}", s.user.as_deref().unwrap_or(""), s.host);
    [s.name.as_str(), s.host.as_str(), user_host.as_str()]
        .iter()
        .any(|t| t.to_lowercase().contains(&f))
}

impl Picker {
    pub fn new(sessions: Vec<SessionConfig>, error: Option<String>) -> Self {
        Self {
            sessions,
            filter: String::new(),
            sel: 0,
            error,
        }
    }

    /// Índices de las sesiones que cumplen el filtro.
    fn visible(&self) -> Vec<usize> {
        (0..self.sessions.len())
            .filter(|&i| matches(&self.sessions[i], &self.filter))
            .collect()
    }

    pub fn handle_key(&mut self, ev: &KeyEvent) -> PickerOut {
        if ev.kind == KeyEventKind::Release
            || (ev.kind == KeyEventKind::Repeat && matches!(ev.code, KeyCode::Enter | KeyCode::Esc))
        {
            return PickerOut::Pending;
        }
        let ctrl = ev.modifiers.contains(KeyModifiers::CONTROL);
        match ev.code {
            KeyCode::Esc => return PickerOut::Cancel,
            KeyCode::Char('c') if ctrl => return PickerOut::Cancel,
            KeyCode::Up => self.sel = self.sel.saturating_sub(1),
            KeyCode::Down => self.sel = (self.sel + 1).min(self.visible().len().saturating_sub(1)),
            KeyCode::Backspace => {
                self.filter.pop();
                self.sel = 0;
                self.error = None;
            }
            KeyCode::Char(c) if !ctrl => {
                self.filter.push(c);
                self.sel = 0;
                self.error = None;
            }
            KeyCode::Enter => return self.accept(),
            _ => {}
        }
        PickerOut::Pending
    }

    pub fn paste(&mut self, text: &str) {
        let line = text.lines().next().unwrap_or("");
        self.filter.extend(line.chars().filter(|c| !c.is_control()));
        self.sel = 0;
        self.error = None;
    }

    /// Con coincidencias, la seleccionada; sin ellas, el texto es un destino ad hoc.
    fn accept(&mut self) -> PickerOut {
        if let Some(&i) = self.visible().get(self.sel) {
            return PickerOut::Open(Box::new(self.sessions[i].clone()));
        }
        match resolve_target(&self.sessions, self.filter.trim()) {
            Some(s) if !self.filter.trim().is_empty() => PickerOut::Open(Box::new(s)),
            _ => {
                self.error = Some("destino no válido: use [usuario@]host[:puerto]".into());
                PickerOut::Pending
            }
        }
    }

    /// Líneas del diálogo: filtro, ventana de la lista y ayuda.
    pub fn lines(&self) -> Vec<String> {
        let vis = self.visible();
        let mut v = vec![format!("> {}_", self.filter)];
        let start = self.sel.saturating_sub(ROWS - 1);
        for (n, &i) in vis.iter().enumerate().skip(start).take(ROWS) {
            let s = &self.sessions[i];
            let mark = if n == self.sel { ">" } else { " " };
            let who = s.user.as_deref().map_or(String::new(), |u| format!("{u}@"));
            v.push(format!("{mark} {}  ({who}{}:{})", s.name, s.host, s.port));
        }
        if vis.is_empty() {
            v.push("  (sin coincidencias: Enter abre el texto como destino)".into());
        }
        if let Some(e) = &self.error {
            v.push(e.clone());
        }
        v.push(String::new());
        v.push("Enter abre  Esc cancela  flechas eligen".into());
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(name: &str, host: &str) -> SessionConfig {
        SessionConfig {
            name: name.into(),
            host: host.into(),
            port: 22,
            ..Default::default()
        }
    }
    fn key(c: KeyCode) -> KeyEvent {
        KeyEvent::new(c, KeyModifiers::NONE)
    }
    fn type_str(p: &mut Picker, t: &str) {
        for c in t.chars() {
            p.handle_key(&key(KeyCode::Char(c)));
        }
    }

    fn picker() -> Picker {
        Picker::new(
            vec![s("prod-db", "10.0.0.1"), s("web", "w.example.org")],
            None,
        )
    }

    #[test]
    fn filters_and_opens_selected() {
        let mut p = picker();
        assert_eq!(p.visible().len(), 2);
        type_str(&mut p, "WEB");
        assert_eq!(p.visible(), vec![1]);
        match p.handle_key(&key(KeyCode::Enter)) {
            PickerOut::Open(c) => assert_eq!(c.name, "web"),
            o => panic!("{o:?}"),
        }
    }

    #[test]
    fn arrows_move_selection() {
        let mut p = picker();
        p.handle_key(&key(KeyCode::Down));
        p.handle_key(&key(KeyCode::Down));
        match p.handle_key(&key(KeyCode::Enter)) {
            PickerOut::Open(c) => assert_eq!(c.name, "web"),
            o => panic!("{o:?}"),
        }
    }

    #[test]
    fn no_match_resolves_adhoc_target_or_errors() {
        let mut p = picker();
        type_str(&mut p, "root@otro.host:2200");
        match p.handle_key(&key(KeyCode::Enter)) {
            PickerOut::Open(c) => {
                assert_eq!((c.host.as_str(), c.port), ("otro.host", 2200));
                assert_eq!(c.user.as_deref(), Some("root"));
            }
            o => panic!("{o:?}"),
        }
        let mut p = picker();
        type_str(&mut p, "no valido");
        assert_eq!(p.handle_key(&key(KeyCode::Enter)), PickerOut::Pending);
        assert!(p.error.is_some());
        p.handle_key(&key(KeyCode::Backspace));
        assert!(p.error.is_none());
        assert_eq!(p.handle_key(&key(KeyCode::Esc)), PickerOut::Cancel);
    }

    #[test]
    fn empty_filter_with_no_sessions_is_an_error() {
        let mut p = Picker::new(vec![], None);
        assert_eq!(p.handle_key(&key(KeyCode::Enter)), PickerOut::Pending);
        assert!(p.error.is_some());
    }
}
