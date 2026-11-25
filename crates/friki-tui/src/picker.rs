//! Selector de sesión: árbol de carpetas (`session_tree`) con las sesiones guardadas,
//! filtrable escribiendo (sobre ruta + nombre, host y usuario), o un destino
//! `[user@]host[:port]`. Estado puro, sin E/S.

use std::collections::HashSet;

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use friki_core::config::{Protocol, SessionConfig, resolve_target, session_tree};

use crate::tag;

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
    /// Carpetas plegadas (ruta completa).
    collapsed: HashSet<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Row {
    Folder { path: String, depth: usize },
    Session { idx: usize, depth: usize },
}

/// `carpeta/nombre`, o solo el nombre en la raíz.
fn full_name(s: &SessionConfig) -> String {
    let folder = friki_core::config::normalize_folder(&s.folder).unwrap_or_default();
    if folder.is_empty() {
        s.name.clone()
    } else {
        format!("{folder}/{}", s.name)
    }
}

fn matches(s: &SessionConfig, f: &str) -> bool {
    let f = f.to_lowercase();
    let user_host = format!("{}@{}", s.user.as_deref().unwrap_or(""), s.host);
    [
        full_name(s).as_str(),
        s.host.as_str(),
        user_host.as_str(),
        tag(s.protocol),
    ]
    .iter()
    .any(|t| t.to_lowercase().contains(&f))
}

/// Destino entre paréntesis según el protocolo.
fn detail(s: &SessionConfig) -> String {
    match s.protocol {
        Protocol::Serial => format!("{}@{}", s.serial.device, s.serial.baud),
        Protocol::LocalShell => s.local_command.clone().unwrap_or_else(|| "shell".into()),
        _ => {
            let who = s.user.as_deref().map_or(String::new(), |u| format!("{u}@"));
            format!("{who}{}:{}", s.host, s.port)
        }
    }
}

impl Picker {
    pub fn new(sessions: Vec<SessionConfig>, error: Option<String>) -> Self {
        Self {
            sessions,
            filter: String::new(),
            sel: 0,
            error,
            collapsed: HashSet::new(),
        }
    }

    fn hidden(&self, folder: &str, include_self: bool) -> bool {
        let mut acc = String::new();
        let segs: Vec<&str> = folder.split('/').filter(|s| !s.is_empty()).collect();
        let n = if include_self {
            segs.len()
        } else {
            segs.len().saturating_sub(1)
        };
        for seg in segs.into_iter().take(n) {
            if !acc.is_empty() {
                acc.push('/');
            }
            acc.push_str(seg);
            if self.collapsed.contains(&acc) {
                return true;
            }
        }
        false
    }

    /// Filas visibles. Con filtro, solo las sesiones que lo cumplen (planas); sin filtro,
    /// el árbol con las carpetas plegadas ocultando su contenido.
    fn rows(&self) -> Vec<Row> {
        let filtering = !self.filter.trim().is_empty();
        let mut out = Vec::new();
        for (folder, list) in session_tree(&self.sessions) {
            let depth = folder.split('/').filter(|s| !s.is_empty()).count();
            if !filtering {
                if !folder.is_empty() {
                    if self.hidden(&folder, false) {
                        continue;
                    }
                    out.push(Row::Folder {
                        path: folder.clone(),
                        depth: depth - 1,
                    });
                }
                if self.hidden(&folder, true) {
                    continue;
                }
            }
            for s in list {
                let Some(idx) = self.sessions.iter().position(|x| std::ptr::eq(x, s)) else {
                    continue;
                };
                if !filtering || matches(s, self.filter.trim()) {
                    out.push(Row::Session {
                        idx,
                        depth: if filtering { 0 } else { depth },
                    });
                }
            }
        }
        out
    }

    fn clamp(&mut self) {
        self.sel = self.sel.min(self.rows().len().saturating_sub(1));
    }

    fn set_collapsed(&mut self, path: &str, on: bool) {
        if on {
            self.collapsed.insert(path.to_string());
        } else {
            self.collapsed.remove(path);
        }
        // La selección sigue a la carpeta aunque cambie la lista.
        if let Some(i) = self
            .rows()
            .iter()
            .position(|r| matches!(r, Row::Folder { path: p, .. } if p == path))
        {
            self.sel = i;
        }
        self.clamp();
    }

    /// Carpeta de la fila seleccionada (la propia si es carpeta; la de la sesión si no).
    fn selected_folder(&self) -> Option<String> {
        match self.rows().get(self.sel)? {
            Row::Folder { path, .. } => Some(path.clone()),
            Row::Session { idx, .. } => {
                let f = friki_core::config::normalize_folder(&self.sessions[*idx].folder)?;
                (!f.is_empty()).then_some(f)
            }
        }
    }

    pub fn handle_key(&mut self, ev: &KeyEvent) -> PickerOut {
        if ev.kind == KeyEventKind::Release
            || (ev.kind == KeyEventKind::Repeat && matches!(ev.code, KeyCode::Enter | KeyCode::Esc))
        {
            return PickerOut::Pending;
        }
        let ctrl = ev.modifiers.contains(KeyModifiers::CONTROL);
        let browsing = self.filter.trim().is_empty();
        match ev.code {
            KeyCode::Esc => return PickerOut::Cancel,
            KeyCode::Char('c') if ctrl => return PickerOut::Cancel,
            KeyCode::Up => self.sel = self.sel.saturating_sub(1),
            KeyCode::Down => self.sel = (self.sel + 1).min(self.rows().len().saturating_sub(1)),
            KeyCode::Left if browsing => {
                if let Some(f) = self.selected_folder() {
                    self.set_collapsed(&f, true);
                }
            }
            KeyCode::Right if browsing => {
                if let Some(Row::Folder { path, .. }) = self.rows().get(self.sel).cloned() {
                    self.set_collapsed(&path, false);
                }
            }
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

    /// Sesión seleccionada: se abre; carpeta: se pliega o despliega; sin filas, el texto
    /// es un destino ad hoc.
    fn accept(&mut self) -> PickerOut {
        match self.rows().get(self.sel).cloned() {
            Some(Row::Session { idx, .. }) => {
                return PickerOut::Open(Box::new(self.sessions[idx].clone()));
            }
            Some(Row::Folder { path, .. }) => {
                let on = !self.collapsed.contains(&path);
                self.set_collapsed(&path, on);
                return PickerOut::Pending;
            }
            None => {}
        }
        match resolve_target(&self.sessions, self.filter.trim()) {
            Some(s) if !self.filter.trim().is_empty() => PickerOut::Open(Box::new(s)),
            _ => {
                self.error = Some("destino no válido: use [usuario@]host[:puerto]".into());
                PickerOut::Pending
            }
        }
    }

    /// Líneas del diálogo: filtro, ventana del árbol y ayuda.
    pub fn lines(&self) -> Vec<String> {
        let rows = self.rows();
        let mut v = vec![format!("> {}_", self.filter)];
        let start = self.sel.saturating_sub(ROWS - 1);
        for (n, row) in rows.iter().enumerate().skip(start).take(ROWS) {
            let mark = if n == self.sel { ">" } else { " " };
            match row {
                Row::Folder { path, depth } => {
                    let open = if self.collapsed.contains(path) {
                        "[+]"
                    } else {
                        "[-]"
                    };
                    let name = path.rsplit('/').next().unwrap_or(path);
                    v.push(format!("{mark} {}{open} {name}/", "  ".repeat(*depth)));
                }
                Row::Session { idx, depth } => {
                    let s = &self.sessions[*idx];
                    let name = if *depth == 0 && !self.filter.trim().is_empty() {
                        full_name(s)
                    } else {
                        s.name.clone()
                    };
                    v.push(format!(
                        "{mark} {}{name}  [{}]  ({})",
                        "  ".repeat(*depth),
                        tag(s.protocol),
                        detail(s)
                    ));
                }
            }
        }
        if rows.is_empty() {
            v.push("  (sin coincidencias: Enter abre el texto como destino)".into());
        }
        if let Some(e) = &self.error {
            v.push(e.clone());
        }
        v.push(String::new());
        v.push("Enter abre  Esc cancela  flechas eligen  izq/der pliegan".into());
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
    fn sf(name: &str, folder: &str, p: Protocol) -> SessionConfig {
        SessionConfig {
            folder: folder.into(),
            protocol: p,
            ..s(name, "10.0.0.9")
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
    fn text(p: &Picker) -> String {
        p.lines().join("\n")
    }

    fn picker() -> Picker {
        Picker::new(
            vec![s("prod-db", "10.0.0.1"), s("web", "w.example.org")],
            None,
        )
    }

    fn tree() -> Picker {
        Picker::new(
            vec![
                s("raiz", "r"),
                sf("db", "clientes/acme", Protocol::Ssh),
                sf("router", "clientes/acme", Protocol::Telnet),
                sf("consola", "lab", Protocol::Serial),
            ],
            None,
        )
    }

    #[test]
    fn filters_and_opens_selected() {
        let mut p = picker();
        assert_eq!(p.rows().len(), 2);
        type_str(&mut p, "WEB");
        assert_eq!(p.rows(), vec![Row::Session { idx: 1, depth: 0 }]);
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

    #[test]
    fn tree_shows_folders_and_protocols() {
        let t = text(&tree());
        let order = [
            "[-] clientes/",
            "[-] acme/",
            "db  [ssh]",
            "router  [telnet]",
        ];
        let pos: Vec<usize> = order.iter().map(|o| t.find(o).expect(o)).collect();
        assert!(pos.windows(2).all(|w| w[0] < w[1]), "{t}");
        assert!(
            t.contains("[-] lab/") && t.contains("consola  [serie]"),
            "{t}"
        );
        assert!(t.contains("raiz  [ssh]"), "{t}");
        // Raíz primero, luego las carpetas.
        assert!(t.find("raiz").expect("raiz") < t.find("clientes/").expect("clientes"));
    }

    #[test]
    fn collapse_and_expand_with_arrows() {
        let mut p = tree();
        // Filas: raiz, clientes/, acme/, db, router, lab/, consola
        assert_eq!(p.rows().len(), 7);
        p.handle_key(&key(KeyCode::Down)); // clientes/
        p.handle_key(&key(KeyCode::Left));
        assert_eq!(p.rows().len(), 4); // raiz, clientes/ (plegada), lab/, consola
        assert!(text(&p).contains("[+] clientes/"));
        p.handle_key(&key(KeyCode::Right));
        assert_eq!(p.rows().len(), 7);
        // Enter sobre una carpeta la pliega; sobre una sesión, la abre.
        assert_eq!(p.handle_key(&key(KeyCode::Enter)), PickerOut::Pending);
        assert_eq!(p.rows().len(), 4);
        // Plegar desde una sesión pliega su carpeta y se queda en ella.
        let mut p = tree();
        for _ in 0..3 {
            p.handle_key(&key(KeyCode::Down)); // db
        }
        p.handle_key(&key(KeyCode::Left));
        assert!(text(&p).contains("[+] acme/"));
        assert_eq!(p.rows().len(), 5); // raiz, clientes/, acme/, lab/, consola
        assert_eq!(p.sel, 2);
    }

    #[test]
    fn filter_matches_path_name_and_protocol() {
        let mut p = tree();
        type_str(&mut p, "acme/r");
        assert_eq!(p.rows(), vec![Row::Session { idx: 2, depth: 0 }]);
        assert!(text(&p).contains("clientes/acme/router  [telnet]"));
        match p.handle_key(&key(KeyCode::Enter)) {
            PickerOut::Open(c) => assert_eq!(c.protocol, Protocol::Telnet),
            o => panic!("{o:?}"),
        }
        let mut p = tree();
        type_str(&mut p, "serie");
        assert_eq!(p.rows().len(), 1);
        // Las flechas laterales no hacen nada mientras se filtra.
        p.handle_key(&key(KeyCode::Left));
        assert_eq!(p.rows().len(), 1);
    }
}
