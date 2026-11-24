//! Analizador IAC y negociación de opciones (sin E/S, para poder probarlo).

pub const IAC: u8 = 255;
const DONT: u8 = 254;
const DO: u8 = 253;
const WONT: u8 = 252;
const WILL: u8 = 251;
const SB: u8 = 250;
const SE: u8 = 240;

pub const OPT_BINARY: u8 = 0;
pub const OPT_ECHO: u8 = 1;
pub const OPT_SGA: u8 = 3;
pub const OPT_TTYPE: u8 = 24;
pub const OPT_NAWS: u8 = 31;
pub const OPT_LFLOW: u8 = 33;

/// Límite de una subnegociación; lo que pase se descarta (evita crecer sin fin).
const SB_MAX: usize = 512;

#[derive(Debug, PartialEq, Eq)]
pub enum Item {
    Data(Vec<u8>),
    /// WILL/WONT/DO/DONT con su opción.
    Neg(u8, u8),
    /// Contenido de IAC SB … IAC SE (sin los delimitadores).
    Sub(Vec<u8>),
}

#[derive(Default)]
enum State {
    #[default]
    Data,
    Iac,
    Neg(u8),
    Sb,
    SbIac,
}

#[derive(Default)]
pub struct Parser {
    state: State,
    sb: Vec<u8>,
}

impl Parser {
    pub fn feed(&mut self, input: &[u8]) -> Vec<Item> {
        let mut out = Vec::new();
        let mut data = Vec::new();
        for &b in input {
            match self.state {
                State::Data if b == IAC => self.state = State::Iac,
                State::Data => data.push(b),
                State::Iac => {
                    self.state = State::Data;
                    match b {
                        IAC => data.push(IAC),
                        WILL | WONT | DO | DONT => self.state = State::Neg(b),
                        SB => {
                            self.sb.clear();
                            self.state = State::Sb;
                        }
                        _ => {} // NOP, GA, DM… no hacen falta
                    }
                }
                State::Neg(cmd) => {
                    if !data.is_empty() {
                        out.push(Item::Data(std::mem::take(&mut data)));
                    }
                    out.push(Item::Neg(cmd, b));
                    self.state = State::Data;
                }
                State::Sb if b == IAC => self.state = State::SbIac,
                State::Sb => self.push_sb(b),
                State::SbIac => match b {
                    SE => {
                        if !data.is_empty() {
                            out.push(Item::Data(std::mem::take(&mut data)));
                        }
                        out.push(Item::Sub(std::mem::take(&mut self.sb)));
                        self.state = State::Data;
                    }
                    IAC => {
                        self.push_sb(IAC);
                        self.state = State::Sb;
                    }
                    _ => self.state = State::Sb,
                },
            }
        }
        if !data.is_empty() {
            out.push(Item::Data(data));
        }
        out
    }

    fn push_sb(&mut self, b: u8) {
        if self.sb.len() < SB_MAX {
            self.sb.push(b);
        }
    }
}

fn us_supported(o: u8) -> bool {
    matches!(o, OPT_BINARY | OPT_SGA | OPT_TTYPE | OPT_NAWS | OPT_LFLOW)
}

fn him_supported(o: u8) -> bool {
    matches!(o, OPT_BINARY | OPT_ECHO | OPT_SGA)
}

pub struct Session {
    term: Vec<u8>,
    cols: u16,
    rows: u16,
    us: [bool; 256],
    him: [bool; 256],
    // Rechazos ya enviados: una opción no soportada se rechaza una sola vez.
    refused_us: [bool; 256],
    refused_him: [bool; 256],
    prev_cr: bool,
}

impl Session {
    pub fn new(term: &str, cols: u16, rows: u16) -> Self {
        Self {
            term: term.bytes().filter(|&b| b != IAC).collect(),
            cols,
            rows,
            us: [false; 256],
            him: [false; 256],
            refused_us: [false; 256],
            refused_him: [false; 256],
            prev_cr: false,
        }
    }

    /// Devuelve (datos para la TUI, bytes de respuesta para el servidor).
    pub fn process(&mut self, items: Vec<Item>) -> (Vec<u8>, Vec<u8>) {
        let (mut data, mut reply) = (Vec::new(), Vec::new());
        for item in items {
            match item {
                Item::Data(d) => self.push_data(&d, &mut data),
                Item::Neg(cmd, o) => self.negotiate(cmd, o, &mut reply),
                Item::Sub(sb) => {
                    // TTYPE SEND (1) -> TTYPE IS (0) <term>
                    if self.us[OPT_TTYPE as usize] && sb.as_slice() == [OPT_TTYPE, 1] {
                        reply.extend([IAC, SB, OPT_TTYPE, 0]);
                        reply.extend(&self.term);
                        reply.extend([IAC, SE]);
                    }
                }
            }
        }
        (data, reply)
    }

    fn push_data(&mut self, d: &[u8], out: &mut Vec<u8>) {
        if self.him[OPT_BINARY as usize] {
            out.extend(d);
            return;
        }
        for &b in d {
            // RFC 854: CR NUL equivale a CR.
            if !(self.prev_cr && b == 0) {
                out.push(b);
            }
            self.prev_cr = b == b'\r';
        }
    }

    fn negotiate(&mut self, cmd: u8, o: u8, reply: &mut Vec<u8>) {
        let i = o as usize;
        match cmd {
            DO if us_supported(o) => {
                if !self.us[i] {
                    self.us[i] = true;
                    reply.extend([IAC, WILL, o]);
                    if o == OPT_NAWS {
                        reply.extend(self.naws());
                    }
                }
            }
            DO => {
                if !std::mem::replace(&mut self.refused_us[i], true) {
                    reply.extend([IAC, WONT, o]);
                }
            }
            DONT => {
                if std::mem::replace(&mut self.us[i], false) {
                    reply.extend([IAC, WONT, o]);
                }
            }
            WILL if him_supported(o) => {
                if !self.him[i] {
                    self.him[i] = true;
                    reply.extend([IAC, DO, o]);
                }
            }
            WILL => {
                if !std::mem::replace(&mut self.refused_him[i], true) {
                    reply.extend([IAC, DONT, o]);
                }
            }
            WONT if std::mem::replace(&mut self.him[i], false) => {
                reply.extend([IAC, DONT, o]);
            }
            _ => {}
        }
    }

    fn naws(&self) -> Vec<u8> {
        let mut v = vec![IAC, SB, OPT_NAWS];
        for b in [self.cols.to_be_bytes(), self.rows.to_be_bytes()].concat() {
            v.push(b);
            if b == IAC {
                v.push(IAC);
            }
        }
        v.extend([IAC, SE]);
        v
    }

    /// Guarda el tamaño; devuelve el SB NAWS si el servidor aceptó la opción.
    pub fn resize(&mut self, cols: u16, rows: u16) -> Option<Vec<u8>> {
        self.cols = cols;
        self.rows = rows;
        self.us[OPT_NAWS as usize].then(|| self.naws())
    }

    /// Escapa IAC y, sin BINARY, convierte CR en CR NUL.
    pub fn encode(&self, data: &[u8]) -> Vec<u8> {
        let binary = self.us[OPT_BINARY as usize];
        let mut v = Vec::with_capacity(data.len() + 2);
        for &b in data {
            v.push(b);
            if b == IAC {
                v.push(IAC);
            } else if b == b'\r' && !binary {
                v.push(0);
            }
        }
        v
    }
}
