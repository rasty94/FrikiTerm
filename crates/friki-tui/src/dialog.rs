//! Diálogos de la TUI: estado puro, sin E/S. El bucle principal los dibuja y les pasa teclas.

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use friki_ssh::HostKeyDecision;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Answer {
    HostKey(HostKeyDecision),
    /// Contraseña o passphrase; `None` si se cancela.
    Secret(Option<String>),
    /// Keyboard-interactive; `None` si se cancela.
    Multi(Option<Vec<String>>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Field {
    pub label: String,
    pub echo: bool,
    pub value: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// Clave desconocida: y / o / n.
    Trust,
    /// Clave cambiada: hay que escribir "yes".
    Changed,
    /// Uno o varios campos de texto.
    Input { multi: bool },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dialog {
    kind: Kind,
    pub title: String,
    pub body: Vec<String>,
    pub fields: Vec<Field>,
    pub focus: usize,
    /// El aviso de clave cambiada se pinta en rojo.
    pub alert: bool,
}

fn field(label: &str, echo: bool) -> Field {
    Field {
        label: label.to_string(),
        echo,
        value: String::new(),
    }
}

impl Dialog {
    pub fn unknown_host(host: &str, port: u16, algorithm: &str, fingerprint: &str) -> Self {
        Self {
            kind: Kind::Trust,
            title: "Clave de host desconocida".into(),
            body: vec![
                format!("Host:      {host}:{port}"),
                format!("Algoritmo: {algorithm}"),
                format!("Huella:    {fingerprint}"),
                String::new(),
                "[y] guardar y continuar   [o] solo esta vez   [n/Esc] cancelar".into(),
            ],
            fields: vec![],
            focus: 0,
            alert: false,
        }
    }

    pub fn changed_host(
        host: &str,
        port: u16,
        algorithm: &str,
        fingerprint: &str,
        previous: &str,
    ) -> Self {
        Self {
            kind: Kind::Changed,
            title: "AVISO: LA CLAVE DEL HOST HA CAMBIADO".into(),
            body: vec![
                "Podría tratarse de un ataque de intermediario (MITM).".into(),
                format!("Host:      {host}:{port}"),
                format!("Algoritmo: {algorithm}"),
                format!("Huella nueva:    {fingerprint}"),
                format!("Huella anterior: {previous}"),
                String::new(),
                "Esc cancela. Para aceptar solo esta vez escriba yes y pulse Enter.".into(),
            ],
            fields: vec![field("> ", true)],
            focus: 0,
            alert: true,
        }
    }

    pub fn password(user: &str, host: &str) -> Self {
        Self::input(
            format!("Contraseña de {user}@{host}"),
            vec![],
            vec![field("Contraseña: ", false)],
            false,
        )
    }

    pub fn passphrase(key_path: &str) -> Self {
        Self::input(
            format!("Passphrase de {key_path}"),
            vec![],
            vec![field("Passphrase: ", false)],
            false,
        )
    }

    pub fn keyboard_interactive(
        name: &str,
        instructions: &str,
        prompts: &[(String, bool)],
    ) -> Self {
        let title = if name.is_empty() {
            "Autenticación".to_string()
        } else {
            name.to_string()
        };
        let body = instructions.lines().map(str::to_string).collect();
        let fields = prompts.iter().map(|(t, echo)| field(t, *echo)).collect();
        Self::input(title, body, fields, true)
    }

    fn input(title: String, body: Vec<String>, fields: Vec<Field>, multi: bool) -> Self {
        Self {
            kind: Kind::Input { multi },
            title,
            body,
            fields,
            focus: 0,
            alert: false,
        }
    }

    /// Procesa una tecla; `Some` cuando el diálogo termina.
    pub fn handle_key(&mut self, ev: &KeyEvent) -> Option<Answer> {
        if ev.kind == KeyEventKind::Release {
            return None;
        }
        let ctrl_c = ev.modifiers.contains(KeyModifiers::CONTROL) && ev.code == KeyCode::Char('c');
        match self.kind {
            Kind::Trust => match ev.code {
                _ if ctrl_c => Some(Answer::HostKey(HostKeyDecision::Reject)),
                KeyCode::Char('y' | 'Y') => Some(Answer::HostKey(HostKeyDecision::AcceptAndSave)),
                KeyCode::Char('o' | 'O') => Some(Answer::HostKey(HostKeyDecision::AcceptOnce)),
                KeyCode::Char('n' | 'N') | KeyCode::Esc => {
                    Some(Answer::HostKey(HostKeyDecision::Reject))
                }
                _ => None,
            },
            Kind::Changed => {
                if ctrl_c || ev.code == KeyCode::Esc {
                    return Some(Answer::HostKey(HostKeyDecision::Reject));
                }
                if ev.code == KeyCode::Enter {
                    let ok = self.fields.first().is_some_and(|f| f.value == "yes");
                    return ok.then_some(Answer::HostKey(HostKeyDecision::AcceptOnce));
                }
                self.edit(ev);
                None
            }
            Kind::Input { multi } => {
                if ctrl_c || ev.code == KeyCode::Esc {
                    return Some(self.cancel_answer());
                }
                if ev.code == KeyCode::Enter {
                    if self.focus + 1 < self.fields.len() {
                        self.focus += 1;
                        return None;
                    }
                    return Some(if multi {
                        Answer::Multi(Some(self.fields.iter().map(|f| f.value.clone()).collect()))
                    } else {
                        Answer::Secret(Some(
                            self.fields
                                .first()
                                .map(|f| f.value.clone())
                                .unwrap_or_default(),
                        ))
                    });
                }
                self.edit(ev);
                None
            }
        }
    }

    fn edit(&mut self, ev: &KeyEvent) {
        let Some(f) = self.fields.get_mut(self.focus) else {
            return;
        };
        match ev.code {
            KeyCode::Char(c) if !ev.modifiers.contains(KeyModifiers::CONTROL) => f.value.push(c),
            KeyCode::Backspace => {
                f.value.pop();
            }
            _ => {}
        }
    }

    /// Texto visible de un campo: sin el valor si no hay eco.
    pub fn shown(&self, i: usize) -> String {
        match self.fields.get(i) {
            Some(f) if f.echo => format!("{}{}", f.label, f.value),
            Some(f) => f.label.clone(),
            None => String::new(),
        }
    }

    /// Respuesta de cancelación (también si se aborta toda la conexión).
    pub fn cancel_answer(&self) -> Answer {
        match self.kind {
            Kind::Trust | Kind::Changed => Answer::HostKey(HostKeyDecision::Reject),
            Kind::Input { multi: true } => Answer::Multi(None),
            Kind::Input { multi: false } => Answer::Secret(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(c: KeyCode) -> KeyEvent {
        KeyEvent::new(c, KeyModifiers::NONE)
    }
    fn type_str(d: &mut Dialog, s: &str) {
        for c in s.chars() {
            assert!(d.handle_key(&key(KeyCode::Char(c))).is_none());
        }
    }

    #[test]
    fn unknown_host_choices() {
        let mk = || Dialog::unknown_host("h", 22, "ssh-ed25519", "SHA256:x");
        assert_eq!(
            mk().handle_key(&key(KeyCode::Char('y'))),
            Some(Answer::HostKey(HostKeyDecision::AcceptAndSave))
        );
        assert_eq!(
            mk().handle_key(&key(KeyCode::Char('o'))),
            Some(Answer::HostKey(HostKeyDecision::AcceptOnce))
        );
        assert_eq!(
            mk().handle_key(&key(KeyCode::Esc)),
            Some(Answer::HostKey(HostKeyDecision::Reject))
        );
        // Enter no decide nada: evita aceptar por accidente.
        assert_eq!(mk().handle_key(&key(KeyCode::Enter)), None);
    }

    #[test]
    fn changed_host_requires_yes() {
        let mut d = Dialog::changed_host("h", 22, "ssh-ed25519", "SHA256:n", "SHA256:o");
        assert!(d.alert);
        assert_eq!(d.handle_key(&key(KeyCode::Char('y'))), None);
        assert_eq!(d.handle_key(&key(KeyCode::Enter)), None);
        type_str(&mut d, "es");
        assert_eq!(
            d.handle_key(&key(KeyCode::Enter)),
            Some(Answer::HostKey(HostKeyDecision::AcceptOnce))
        );
        let mut d = Dialog::changed_host("h", 22, "a", "n", "o");
        type_str(&mut d, "ye");
        assert_eq!(d.handle_key(&key(KeyCode::Enter)), None);
        let mut d = Dialog::changed_host("h", 22, "a", "n", "o");
        type_str(&mut d, "yes");
        assert_eq!(
            d.handle_key(&key(KeyCode::Esc)),
            Some(Answer::HostKey(HostKeyDecision::Reject))
        );
    }

    #[test]
    fn password_no_echo_and_cancel() {
        let mut d = Dialog::password("u", "h");
        type_str(&mut d, "abc");
        d.handle_key(&key(KeyCode::Backspace));
        assert_eq!(d.shown(0), "Contraseña: ");
        assert_eq!(
            d.handle_key(&key(KeyCode::Enter)),
            Some(Answer::Secret(Some("ab".into())))
        );
        let mut d = Dialog::passphrase("/k");
        assert_eq!(d.handle_key(&key(KeyCode::Esc)), Some(Answer::Secret(None)));
    }

    #[test]
    fn keyboard_interactive_respects_echo() {
        let prompts = vec![("User: ".to_string(), true), ("Code: ".to_string(), false)];
        let mut d = Dialog::keyboard_interactive("2fa", "line1\nline2", &prompts);
        assert_eq!(d.body.len(), 2);
        type_str(&mut d, "bob");
        assert_eq!(d.shown(0), "User: bob");
        assert!(d.handle_key(&key(KeyCode::Enter)).is_none());
        type_str(&mut d, "123");
        assert_eq!(d.shown(1), "Code: ");
        assert_eq!(
            d.handle_key(&key(KeyCode::Enter)),
            Some(Answer::Multi(Some(vec!["bob".into(), "123".into()])))
        );
        let mut d = Dialog::keyboard_interactive("", "", &prompts);
        assert_eq!(d.handle_key(&key(KeyCode::Esc)), Some(Answer::Multi(None)));
    }
}
