//! Negociación Telnet para TN3270 (RFC 854/855, 1091 TERMINAL-TYPE, 856 BINARY, 885 EOR,
//! RFC 1576) y TN3270E básico sin funciones (RFC 2355). Parser incremental con límites.

const IAC: u8 = 255;
const DONT: u8 = 254;
const DO: u8 = 253;
const WONT: u8 = 252;
const WILL: u8 = 251;
const SB: u8 = 250;
const SE: u8 = 240;
const EOR_CMD: u8 = 239;

const OPT_BINARY: u8 = 0;
const OPT_TTYPE: u8 = 24;
const OPT_EOR: u8 = 25;
const OPT_TN3270E: u8 = 40;

/// Tope de un registro 3270 (el resto se descarta hasta el EOR).
pub const MAX_RECORD: usize = 1 << 20;
const MAX_SUB: usize = 512;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum St {
    Data,
    Iac,
    Neg(u8),
    Sb,
    SbIac,
}

#[derive(Debug)]
pub struct Telnet {
    st: St,
    buf: Vec<u8>,
    overflow: bool,
    sub: Vec<u8>,
    us: [bool; 256],
    them: [bool; 256],
    term: String,
    tn3270e_allowed: bool,
    /// TN3270E: el servidor aceptó el dispositivo y los registros llevan cabecera.
    e_ready: bool,
}

impl Telnet {
    pub fn new(term: &str, tn3270e: bool) -> Self {
        Self {
            st: St::Data,
            buf: Vec::new(),
            overflow: false,
            sub: Vec::new(),
            us: [false; 256],
            them: [false; 256],
            term: term.to_string(),
            tn3270e_allowed: tn3270e,
            e_ready: false,
        }
    }

    /// `true` si el modo es TN3270E con cabecera de 5 bytes.
    pub fn is_tn3270e(&self) -> bool {
        self.e_ready
    }

    /// Opciones BINARY y EOR activas en ambos sentidos (modo TN3270 clásico).
    pub fn is_binary_eor(&self) -> bool {
        let both = |o: u8| self.us[usize::from(o)] && self.them[usize::from(o)];
        both(OPT_BINARY) && both(OPT_EOR)
    }

    /// Consume bytes del servidor. Las respuestas de negociación se añaden a `reply` y los
    /// registros 3270 completos a `records`.
    pub fn feed(&mut self, input: &[u8], reply: &mut Vec<u8>, records: &mut Vec<Vec<u8>>) {
        for &b in input {
            match self.st {
                St::Data => {
                    if b == IAC {
                        self.st = St::Iac;
                    } else {
                        self.push(b);
                    }
                }
                St::Iac => {
                    self.st = St::Data;
                    match b {
                        IAC => self.push(IAC),
                        EOR_CMD => self.end_record(records),
                        WILL | WONT | DO | DONT => self.st = St::Neg(b),
                        SB => {
                            self.sub.clear();
                            self.st = St::Sb;
                        }
                        _ => {}
                    }
                }
                St::Neg(cmd) => {
                    self.st = St::Data;
                    self.negotiate(cmd, b, reply);
                }
                St::Sb => {
                    if b == IAC {
                        self.st = St::SbIac;
                    } else {
                        self.push_sub(b);
                    }
                }
                St::SbIac => match b {
                    SE => {
                        self.st = St::Data;
                        let sub = std::mem::take(&mut self.sub);
                        self.subnegotiate(&sub, reply);
                    }
                    IAC => {
                        self.push_sub(IAC);
                        self.st = St::Sb;
                    }
                    _ => self.st = St::Sb,
                },
            }
        }
    }

    fn push(&mut self, b: u8) {
        if self.buf.len() < MAX_RECORD {
            self.buf.push(b);
        } else {
            self.overflow = true;
        }
    }

    fn push_sub(&mut self, b: u8) {
        if self.sub.len() < MAX_SUB {
            self.sub.push(b);
        }
    }

    fn end_record(&mut self, records: &mut Vec<Vec<u8>>) {
        let rec = std::mem::take(&mut self.buf);
        if std::mem::take(&mut self.overflow) {
            return;
        }
        if self.e_ready {
            // cabecera TN3270E: tipo, petición, respuesta, secuencia(2); solo 3270-DATA (0)
            if rec.len() > 5 && rec[0] == 0 {
                records.push(rec[5..].to_vec());
            }
        } else if !rec.is_empty() {
            records.push(rec);
        }
    }

    fn supported(&self, opt: u8) -> bool {
        matches!(opt, OPT_BINARY | OPT_TTYPE | OPT_EOR)
            || (opt == OPT_TN3270E && self.tn3270e_allowed)
    }

    fn negotiate(&mut self, cmd: u8, opt: u8, reply: &mut Vec<u8>) {
        let o = usize::from(opt);
        match cmd {
            DO if self.supported(opt) => {
                if !self.us[o] {
                    self.us[o] = true;
                    reply.extend([IAC, WILL, opt]);
                }
            }
            DO => reply.extend([IAC, WONT, opt]),
            DONT => {
                if self.us[o] {
                    self.us[o] = false;
                    reply.extend([IAC, WONT, opt]);
                }
                if opt == OPT_TN3270E {
                    self.e_ready = false;
                }
            }
            // TTYPE y TN3270E solo los ofrecemos nosotros; el servidor no los "activa"
            WILL if matches!(opt, OPT_BINARY | OPT_EOR) => {
                if !self.them[o] {
                    self.them[o] = true;
                    reply.extend([IAC, DO, opt]);
                }
            }
            WILL => reply.extend([IAC, DONT, opt]),
            _ => {
                if self.them[o] {
                    self.them[o] = false;
                    reply.extend([IAC, DONT, opt]);
                }
            }
        }
    }

    fn sb(&self, payload: &[u8]) -> Vec<u8> {
        let mut v = vec![IAC, SB];
        for &b in payload {
            v.push(b);
            if b == IAC {
                v.push(IAC);
            }
        }
        v.extend([IAC, SE]);
        v
    }

    fn subnegotiate(&mut self, sub: &[u8], reply: &mut Vec<u8>) {
        match sub {
            // TERMINAL-TYPE SEND -> IS <tipo>
            [OPT_TTYPE, 1, ..] if self.us[usize::from(OPT_TTYPE)] => {
                let mut p = vec![OPT_TTYPE, 0];
                p.extend(self.term.bytes());
                reply.extend(self.sb(&p));
            }
            [OPT_TN3270E, rest @ ..] if self.us[usize::from(OPT_TN3270E)] => {
                self.sub_e(rest, reply)
            }
            _ => {}
        }
    }

    /// RFC 2355: DEVICE-TYPE (2) y FUNCTIONS (3); sin funciones opcionales.
    fn sub_e(&mut self, s: &[u8], reply: &mut Vec<u8>) {
        match s {
            // SEND DEVICE-TYPE
            [8, 2] => {
                let mut p = vec![OPT_TN3270E, 2, 7];
                p.extend(self.term.bytes());
                reply.extend(self.sb(&p));
            }
            // DEVICE-TYPE IS <tipo> CONNECT <nombre>
            [2, 4, ..] => {
                reply.extend(self.sb(&[OPT_TN3270E, 3, 7]));
            }
            // DEVICE-TYPE REJECT: volver a TN3270 clásico
            [2, 6, ..] => {
                self.us[usize::from(OPT_TN3270E)] = false;
                reply.extend([IAC, WONT, OPT_TN3270E]);
            }
            // FUNCTIONS REQUEST <lista>: aceptamos solo la lista vacía
            [3, 7, list @ ..] => {
                if list.is_empty() {
                    self.e_ready = true;
                    reply.extend(self.sb(&[OPT_TN3270E, 3, 4]));
                } else {
                    reply.extend(self.sb(&[OPT_TN3270E, 3, 7]));
                }
            }
            // FUNCTIONS IS: el servidor confirma (vacía)
            [3, 4] => self.e_ready = true,
            _ => {}
        }
    }

    /// Registro de salida: cabecera TN3270E si procede, IAC duplicados y IAC EOR.
    pub fn encode_record(&self, data: &[u8]) -> Vec<u8> {
        let mut v = Vec::with_capacity(data.len() + 8);
        let hdr: &[u8] = if self.e_ready { &[0; 5] } else { &[] };
        for &b in hdr.iter().chain(data) {
            v.push(b);
            if b == IAC {
                v.push(IAC);
            }
        }
        v.extend([IAC, EOR_CMD]);
        v
    }
}
