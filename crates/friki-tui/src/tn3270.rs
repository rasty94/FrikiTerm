//! Panel TN3270: el flujo 3270 lo interpreta `friki-tn3270`; aquí solo se enlaza con el
//! emulador del panel. Se pinta con `render::ansi` y las teclas llegan como bytes del
//! emulador, que `keys::decode` traduce a AID y navegación de campos.

use friki_term::Terminal as Emulator;
use friki_tn3270::{Client, Tn3270Options, keys, render};

pub struct TnPane {
    client: Client,
}

impl TnPane {
    pub fn new() -> Self {
        Self {
            client: Client::new(&Tn3270Options::default()),
        }
    }

    /// Bytes del servidor: devuelve lo que hay que responder y repinta `emu`.
    pub fn feed(&mut self, data: &[u8], emu: Option<&mut Emulator>) -> Vec<u8> {
        let out = self.client.feed(data).to_server;
        self.paint(emu);
        out
    }

    /// Bytes del teclado (secuencias del emulador): AID, campos y navegación 3270.
    pub fn key(&mut self, raw: &[u8], emu: Option<&mut Emulator>) -> Vec<u8> {
        let mut out = Vec::new();
        for k in keys::decode(raw) {
            out.extend(self.client.key(k).to_server);
        }
        self.paint(emu);
        out
    }

    fn paint(&self, emu: Option<&mut Emulator>) {
        if let Some(e) = emu {
            e.process(&render::ansi(&self.client.screen));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::TnPane;

    #[test]
    fn enter_del_teclado_produce_registro_3270() {
        let mut tn = TnPane::new();
        // EW + WCC con restablecimiento de teclado, SBA 0, "HI" (EBCDIC) y IAC EOR
        let out = tn.feed(
            &[0xF5, 0xC3, 0x11, 0x40, 0x40, 0xC8, 0xC9, 0xFF, 0xEF],
            None,
        );
        assert!(out.is_empty());
        // Enter del emulador (CR) -> AID Enter + datos + IAC EOR
        let out = tn.key(b"\r", None);
        assert_eq!(out, [0x7D, 0x40, 0x40, 0xC8, 0xC9, 0xFF, 0xEF]);
    }
}
