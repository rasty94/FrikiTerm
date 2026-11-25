//! Diálogos de la TUI: estado puro, sin E/S. El bucle principal los dibuja y les pasa teclas.

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use friki_ssh::{HostKeyDecision, Zeroizing};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Answer {
    HostKey(HostKeyDecision),
    /// Contraseña o passphrase; `None` si se cancela.
    Secret(Option<Zeroizing<String>>),
    /// Keyboard-interactive; `None` si se cancela.
    Multi(Option<Vec<Zeroizing<String>>>),
    /// Confirmaciones y avisos.
    Confirm(bool),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Field {
    pub label: String,
    pub echo: bool,
    pub value: Zeroizing<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// Clave desconocida: hay que escribir "yes" u "once".
    Trust,
    /// Clave cambiada: hay que escribir "yes" (solo esta vez).
    Changed,
    /// Uno o varios campos de texto.
    Input { multi: bool },
    /// s / n.
    Confirm,
    /// Hay que escribir "yes" y Enter (Esc cancela); contesta `Confirm`.
    ConfirmYes,
    /// Aviso: Enter, Esc o espacio lo cierran.
    Notice,
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
    /// Hasta este instante el diálogo descarta teclas y pegados (los provoca el remoto).
    locked_until: Option<Instant>,
}

/// Quita de un texto ajeno (servidor, nombres de algoritmo) los controles C0, DEL y C1,
/// los separadores de línea/párrafo y los controles bidireccionales, que permitirían
/// pintar encima del diálogo o reordenar el texto. El tabulador pasa a espacio.
pub fn clean(s: &str) -> String {
    s.chars()
        .filter_map(|c| match c {
            '\t' => Some(' '),
            c if c.is_control() => None,
            '\u{200b}'..='\u{200f}' | '\u{2028}'..='\u{202e}' | '\u{2066}'..='\u{2069}' => None,
            c => Some(c),
        })
        .collect()
}

fn field(label: &str, echo: bool) -> Field {
    Field {
        label: label.to_string(),
        echo,
        // Capacidad fija para que escribir no reasigne (y deje copias sin borrar).
        value: Zeroizing::new(String::with_capacity(256)),
    }
}

/// Teclas que un diálogo ignora: soltar una tecla y repetir Enter/Esc.
fn ignored(ev: &KeyEvent) -> bool {
    match ev.kind {
        KeyEventKind::Press => false,
        KeyEventKind::Release => true,
        KeyEventKind::Repeat => matches!(ev.code, KeyCode::Enter | KeyCode::Esc),
    }
}

impl Dialog {
    fn new(kind: Kind, title: String, body: Vec<String>, fields: Vec<Field>) -> Self {
        Self {
            kind,
            title,
            body,
            fields,
            focus: 0,
            alert: false,
            locked_until: None,
        }
    }

    /// Descarta la entrada durante `d` desde ahora: un diálogo que abre el remoto no debe
    /// recibir teclas que el usuario tecleaba antes de verlo.
    pub fn locked_for(mut self, d: Duration) -> Self {
        self.locked_until = Some(Instant::now() + d);
        self
    }

    fn is_locked(&self) -> bool {
        self.locked_until.is_some_and(|t| Instant::now() < t)
    }

    #[cfg(test)]
    pub fn unlock(&mut self) {
        self.locked_until = None;
    }

    /// Pide escribir `yes` y Enter (no basta una tecla).
    pub fn confirm_yes(title: &str, question: &str) -> Self {
        Self::new(
            Kind::ConfirmYes,
            title.into(),
            vec![question.into(), "Escriba yes y Enter; Esc cancela.".into()],
            vec![field("> ", true)],
        )
    }

    pub fn unknown_host(host: &str, port: u16, algorithm: &str, fingerprint: &str) -> Self {
        Self::new(
            Kind::Trust,
            "Clave de host desconocida".into(),
            vec![
                format!("Host:      {}:{port}", clean(host)),
                format!("Algoritmo: {}", clean(algorithm)),
                format!("Huella:    {}", clean(fingerprint)),
                String::new(),
                "Escriba yes y Enter para guardar y continuar,".into(),
                "once y Enter para aceptar solo esta vez. Esc cancela.".into(),
            ],
            vec![field("> ", true)],
        )
    }

    pub fn changed_host(
        host: &str,
        port: u16,
        algorithm: &str,
        fingerprint: &str,
        previous: &str,
    ) -> Self {
        let mut d = Self::new(
            Kind::Changed,
            "AVISO: LA CLAVE DEL HOST HA CAMBIADO".into(),
            vec![
                "Podría tratarse de un ataque de intermediario (MITM).".into(),
                format!("Host:      {}:{port}", clean(host)),
                format!("Algoritmo: {}", clean(algorithm)),
                format!("Huella nueva:    {}", clean(fingerprint)),
                format!("Huella anterior: {}", clean(previous)),
                String::new(),
                "Esc cancela. Para aceptar solo esta vez escriba yes y pulse Enter.".into(),
                "La clave nueva no se guarda.".into(),
            ],
            vec![field("> ", true)],
        );
        d.alert = true;
        d
    }

    pub fn password(user: &str, host: &str) -> Self {
        Self::new(
            Kind::Input { multi: false },
            format!("Contraseña de {}@{}", clean(user), clean(host)),
            vec![],
            vec![field("Contraseña: ", false)],
        )
    }

    pub fn passphrase(key_path: &str) -> Self {
        Self::new(
            Kind::Input { multi: false },
            format!("Passphrase de {}", clean(key_path)),
            vec![],
            vec![field("Passphrase: ", false)],
        )
    }

    /// El título es siempre local y fijo; todo lo que envía el servidor (nombre,
    /// instrucciones, textos de los campos) va en el cuerpo, marcado y sin controles,
    /// para que no pueda hacerse pasar por un diálogo local.
    pub fn keyboard_interactive(
        host: &str,
        name: &str,
        instructions: &str,
        prompts: &[(String, bool)],
    ) -> Self {
        let mut body = vec!["Texto enviado por el servidor (no es de confianza):".to_string()];
        if !name.is_empty() {
            body.push(format!("  servidor> nombre: {}", clean(name)));
        }
        body.extend(
            instructions
                .lines()
                .map(|l| format!("  servidor> {}", clean(l))),
        );
        let fields = prompts
            .iter()
            .map(|(t, echo)| field(&format!("[servidor] {}", clean(t)), *echo))
            .collect();
        Self::new(
            Kind::Input { multi: true },
            format!("El servidor {} pregunta", clean(host)),
            body,
            fields,
        )
    }

    pub fn paste_confirm(lines: usize) -> Self {
        Self::new(
            Kind::Confirm,
            "Pegar".into(),
            vec![format!(
                "Pegar {lines} líneas sin bracketed paste? s/n (Esc cancela)"
            )],
            vec![],
        )
    }

    /// Entrada de una línea con eco y valor inicial (renombrar pestaña). Contesta con
    /// `Answer::Secret(Some(texto))`.
    pub fn text(title: &str, label: &str, initial: &str) -> Self {
        let mut f = field(label, true);
        f.value.push_str(&clean(initial));
        Self::new(Kind::Input { multi: false }, title.into(), vec![], vec![f])
    }

    /// Aviso informativo (Enter, Esc o espacio lo cierran).
    pub fn notice(title: &str, lines: &[String], alert: bool) -> Self {
        let mut body: Vec<String> = lines.iter().map(|l| clean(l)).collect();
        body.push(String::new());
        body.push("Enter para cerrar.".into());
        let mut d = Self::new(Kind::Notice, title.into(), body, vec![]);
        d.alert = alert;
        d
    }

    /// Procesa una tecla; `Some` cuando el diálogo termina.
    pub fn handle_key(&mut self, ev: &KeyEvent) -> Option<Answer> {
        if ignored(ev) || self.is_locked() {
            return None;
        }
        let ctrl_c = ev.modifiers.contains(KeyModifiers::CONTROL) && ev.code == KeyCode::Char('c');
        let cancel = ctrl_c || ev.code == KeyCode::Esc;
        match self.kind {
            Kind::Trust | Kind::Changed => {
                if cancel {
                    return Some(Answer::HostKey(HostKeyDecision::Reject));
                }
                if ev.code == KeyCode::Enter {
                    let typed = self.fields.first().map(|f| f.value.as_str());
                    return match (self.kind, typed) {
                        (Kind::Trust, Some("yes")) => {
                            Some(Answer::HostKey(HostKeyDecision::AcceptAndSave))
                        }
                        (Kind::Trust, Some("once")) | (Kind::Changed, Some("yes")) => {
                            Some(Answer::HostKey(HostKeyDecision::AcceptOnce))
                        }
                        _ => None,
                    };
                }
                self.edit(ev);
                None
            }
            Kind::Input { multi } => {
                if cancel {
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
            Kind::ConfirmYes => {
                if cancel {
                    return Some(Answer::Confirm(false));
                }
                if ev.code == KeyCode::Enter {
                    let yes = self
                        .fields
                        .first()
                        .is_some_and(|f| f.value.as_str() == "yes");
                    return yes.then_some(Answer::Confirm(true));
                }
                self.edit(ev);
                None
            }
            Kind::Confirm => match ev.code {
                _ if cancel => Some(Answer::Confirm(false)),
                KeyCode::Char('s' | 'S') => Some(Answer::Confirm(true)),
                KeyCode::Char('n' | 'N') => Some(Answer::Confirm(false)),
                _ => None,
            },
            Kind::Notice => matches!(ev.code, KeyCode::Enter | KeyCode::Esc | KeyCode::Char(' '))
                .then_some(Answer::Confirm(true)),
        }
    }

    /// Texto pegado (bracketed paste) en el campo activo: primera línea, sin controles.
    pub fn paste(&mut self, text: &str) {
        if self.is_locked() {
            return;
        }
        if let Some(f) = self.fields.get_mut(self.focus) {
            let line = text.lines().next().unwrap_or("");
            f.value.extend(line.chars().filter(|c| !c.is_control()));
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
            Some(f) if f.echo => format!("{}{}", f.label, f.value.as_str()),
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
            Kind::Confirm | Kind::ConfirmYes | Kind::Notice => Answer::Confirm(false),
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
    fn secret(s: &str) -> Zeroizing<String> {
        Zeroizing::new(s.to_string())
    }

    #[test]
    fn unknown_host_needs_typed_answer() {
        let mk = || Dialog::unknown_host("h", 22, "ssh-ed25519", "SHA256:x");
        // Una tecla suelta (también 'y' / 'o' / 'n') no decide nada.
        for c in ['y', 'Y', 'o', 'n', ' '] {
            assert_eq!(mk().handle_key(&key(KeyCode::Char(c))), None);
        }
        assert_eq!(mk().handle_key(&key(KeyCode::Enter)), None);
        let mut d = mk();
        type_str(&mut d, "yes");
        assert_eq!(
            d.handle_key(&key(KeyCode::Enter)),
            Some(Answer::HostKey(HostKeyDecision::AcceptAndSave))
        );
        let mut d = mk();
        type_str(&mut d, "once");
        assert_eq!(
            d.handle_key(&key(KeyCode::Enter)),
            Some(Answer::HostKey(HostKeyDecision::AcceptOnce))
        );
        let mut d = mk();
        type_str(&mut d, "ye");
        assert_eq!(d.handle_key(&key(KeyCode::Enter)), None);
        let mut d = mk();
        type_str(&mut d, "yes");
        assert_eq!(
            d.handle_key(&key(KeyCode::Esc)),
            Some(Answer::HostKey(HostKeyDecision::Reject))
        );
    }

    #[test]
    fn release_and_repeat_enter_are_ignored() {
        let mut d = Dialog::unknown_host("h", 22, "a", "f");
        type_str(&mut d, "yes");
        let mut e = key(KeyCode::Enter);
        e.kind = KeyEventKind::Release;
        assert_eq!(d.handle_key(&e), None);
        e.kind = KeyEventKind::Repeat;
        assert_eq!(d.handle_key(&e), None);
        assert!(d.handle_key(&key(KeyCode::Enter)).is_some());
    }

    #[test]
    fn changed_host_requires_yes_and_never_saves() {
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
        type_str(&mut d, "once");
        assert_eq!(d.handle_key(&key(KeyCode::Enter)), None);
        let mut d = Dialog::changed_host("h", 22, "a", "n", "o");
        type_str(&mut d, "yes");
        assert_eq!(
            d.handle_key(&key(KeyCode::Esc)),
            Some(Answer::HostKey(HostKeyDecision::Reject))
        );
    }

    #[test]
    fn password_no_echo_cancel_and_paste() {
        let mut d = Dialog::password("u", "h");
        type_str(&mut d, "abc");
        d.handle_key(&key(KeyCode::Backspace));
        assert_eq!(d.shown(0), "Contraseña: ");
        assert_eq!(
            d.handle_key(&key(KeyCode::Enter)),
            Some(Answer::Secret(Some(secret("ab"))))
        );
        let mut d = Dialog::passphrase("/k");
        d.paste("pega\x1b[31mdo\nsegunda");
        assert_eq!(
            d.handle_key(&key(KeyCode::Enter)),
            Some(Answer::Secret(Some(secret("pega[31mdo"))))
        );
        let mut d = Dialog::passphrase("/k");
        assert_eq!(d.handle_key(&key(KeyCode::Esc)), Some(Answer::Secret(None)));
    }

    #[test]
    fn keyboard_interactive_respects_echo() {
        let prompts = vec![("User: ".to_string(), true), ("Code: ".to_string(), false)];
        let mut d = Dialog::keyboard_interactive("h", "2fa", "line1\nline2", &prompts);
        assert_eq!(d.body.len(), 4);
        type_str(&mut d, "bob");
        assert_eq!(d.shown(0), "[servidor] User: bob");
        assert!(d.handle_key(&key(KeyCode::Enter)).is_none());
        type_str(&mut d, "123");
        assert_eq!(d.shown(1), "[servidor] Code: ");
        assert_eq!(
            d.handle_key(&key(KeyCode::Enter)),
            Some(Answer::Multi(Some(vec![secret("bob"), secret("123")])))
        );
        let mut d = Dialog::keyboard_interactive("h", "", "", &prompts);
        assert_eq!(d.handle_key(&key(KeyCode::Esc)), Some(Answer::Multi(None)));
    }

    #[test]
    fn keyboard_interactive_title_is_local_and_remote_text_is_clean() {
        let name = "Passphrase de ~/.ssh/id_ed25519\x1b[2J\u{9b}\u{202e}";
        let ins = "pon\x07 tu\r clave\t\x7f\n\x1b]0;x\x07fin";
        let prompts = vec![("Clave\x1b[H: ".to_string(), false)];
        let d = Dialog::keyboard_interactive("srv", name, ins, &prompts);
        assert_eq!(d.title, "El servidor srv pregunta");
        assert!(!d.title.contains("Passphrase"));
        let all = d
            .body
            .iter()
            .chain(std::iter::once(&d.title))
            .chain(d.fields.iter().map(|f| &f.label));
        for s in all {
            assert!(
                !s.chars().any(|c| c.is_control() || c == '\u{202e}'),
                "{s:?}"
            );
        }
        assert!(d.body.iter().any(|l| l.contains("Passphrase de ~/.ssh")));
        assert!(d.body[0].contains("servidor"));
        assert!(d.fields[0].label.starts_with("[servidor] "));
    }

    #[test]
    fn paste_confirm_and_notice() {
        let mut d = Dialog::paste_confirm(3);
        assert!(d.body[0].contains("Pegar 3 líneas"));
        assert_eq!(d.handle_key(&key(KeyCode::Enter)), None);
        assert_eq!(
            d.handle_key(&key(KeyCode::Char('s'))),
            Some(Answer::Confirm(true))
        );
        assert_eq!(
            Dialog::paste_confirm(3).handle_key(&key(KeyCode::Char('n'))),
            Some(Answer::Confirm(false))
        );
        assert_eq!(
            Dialog::paste_confirm(3).handle_key(&key(KeyCode::Esc)),
            Some(Answer::Confirm(false))
        );
        let mut n = Dialog::notice("t", &["a\x1b".into()], true);
        assert_eq!(n.handle_key(&key(KeyCode::Char('x'))), None);
        assert_eq!(
            n.handle_key(&key(KeyCode::Enter)),
            Some(Answer::Confirm(true))
        );
    }

    #[test]
    fn confirm_yes_needs_the_word_and_locked_dialogs_drop_input() {
        let mut d = Dialog::confirm_yes("Zmodem", "Recibir?").locked_for(Duration::from_secs(60));
        // Bloqueado: ni teclas ni pegados llegan.
        for c in "yes".chars() {
            assert!(d.handle_key(&key(KeyCode::Char(c))).is_none());
        }
        d.paste("yes");
        assert!(d.fields[0].value.is_empty());
        assert!(d.handle_key(&key(KeyCode::Esc)).is_none());
        d.unlock();
        assert!(d.handle_key(&key(KeyCode::Char('s'))).is_none());
        assert!(
            d.handle_key(&key(KeyCode::Enter)).is_none(),
            "una s no basta"
        );
        d.handle_key(&key(KeyCode::Backspace));
        type_str(&mut d, "yes");
        assert_eq!(
            d.handle_key(&key(KeyCode::Enter)),
            Some(Answer::Confirm(true))
        );
        let mut d = Dialog::confirm_yes("Zmodem", "Recibir?");
        assert_eq!(
            d.handle_key(&key(KeyCode::Esc)),
            Some(Answer::Confirm(false))
        );
    }
}
