//! Cliente TN3270 síncrono y puro: bytes del servidor -> pantalla + bytes de respuesta;
//! teclas -> bytes hacia el servidor. La E/S asíncrona vive en `session`.

use crate::datastream::{ReadKind, Reply};
use crate::keys::Key;
use crate::screen::{Screen, aid};
use crate::telnet::Telnet;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tn3270Options {
    /// Modelo 2..5 (24x80, 32x80, 43x80, 27x132). Anuncia `IBM-3278-<m>-E`.
    pub model: u8,
    /// Ofrecer TN3270E (sin funciones opcionales) si el servidor lo propone.
    pub tn3270e: bool,
    pub connect_timeout_secs: u64,
}

impl Default for Tn3270Options {
    fn default() -> Self {
        Self {
            model: 2,
            tn3270e: false,
            connect_timeout_secs: 15,
        }
    }
}

/// Resultado de procesar entrada: bytes a enviar y si hay que redibujar.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Output {
    pub to_server: Vec<u8>,
    pub redraw: bool,
}

#[derive(Debug)]
pub struct Client {
    pub screen: Screen,
    telnet: Telnet,
    /// Último error de flujo 3270 (diagnóstico).
    pub last_error: Option<crate::datastream::StreamError>,
}

impl Client {
    pub fn new(opts: &Tn3270Options) -> Self {
        let model = opts.model.clamp(2, 5);
        Self {
            screen: Screen::model(model),
            telnet: Telnet::new(&format!("IBM-3278-{model}-E"), opts.tn3270e),
            last_error: None,
        }
    }

    pub fn telnet(&self) -> &crate::telnet::Telnet {
        &self.telnet
    }

    /// Bytes recibidos del servidor.
    pub fn feed(&mut self, bytes: &[u8]) -> Output {
        let mut out = Output::default();
        let mut records = Vec::new();
        self.telnet.feed(bytes, &mut out.to_server, &mut records);
        for rec in records {
            match self.screen.process(&rec) {
                Ok(replies) => {
                    for r in replies {
                        let body = match r {
                            Reply::Query => self.screen.query_reply(),
                            Reply::Read(ReadKind::Buffer) => {
                                self.screen.read_buffer(self.screen.last_aid)
                            }
                            Reply::Read(k) => {
                                let all = k == ReadKind::ModifiedAll;
                                self.screen.read_modified(self.screen.last_aid, all)
                            }
                        };
                        out.to_server.extend(self.telnet.encode_record(&body));
                    }
                }
                Err(e) => self.last_error = Some(e),
            }
            out.redraw = true;
        }
        out
    }

    /// Una tecla del usuario.
    pub fn key(&mut self, k: Key) -> Output {
        let mut out = Output {
            redraw: true,
            ..Output::default()
        };
        let s = &mut self.screen;
        match k {
            Key::Char(c) => {
                let _ = s.type_char(c);
            }
            Key::Aid(a) => {
                if !s.keyboard_locked() || a == aid::SYSREQ {
                    let body = s.press_aid(a);
                    out.to_server = self.telnet.encode_record(&body);
                }
            }
            Key::Tab => s.tab(),
            Key::BackTab => s.backtab(),
            Key::Backspace => {
                let _ = s.backspace();
            }
            Key::Delete => {
                let _ = s.delete();
            }
            Key::EraseEof => {
                let _ = s.erase_eof();
            }
            Key::EraseInput => s.erase_input(),
            Key::Up => s.move_cursor(-1, 0),
            Key::Down => s.move_cursor(1, 0),
            Key::Left => s.move_cursor(0, -1),
            Key::Right => s.move_cursor(0, 1),
            Key::Home => s.home(),
            Key::Reset => s.reset(),
        }
        out
    }
}
