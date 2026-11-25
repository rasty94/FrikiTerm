#![no_main]
use friki_core::config::{Emulation, TerminalProfile};
use friki_term::{Key, Modifiers, Terminal};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // Byte 0: perfil Wyse/TVI; byte 1: punto de corte; resto: flujo de entrada.
    let (head, rest) = data.split_at(data.len().min(2));
    let emulation = [
        Emulation::Wyse50,
        Emulation::Wyse60,
        Emulation::Tvi910,
        Emulation::Tvi925,
    ][usize::from(head.first().copied().unwrap_or(0)) % 4];
    let cut = usize::from(head.get(1).copied().unwrap_or(0)).min(rest.len());
    let profile = TerminalProfile {
        emulation,
        ..Default::default()
    };
    let mut t = Terminal::with_profile(24, 80, &profile);
    t.process(&rest[..cut]);
    t.resize(10, 20);
    t.process(&rest[cut..]);
    let _ = t.take_replies();
    let _ = t.screen();
    let _ = t.encode_key(Key::Char('a'), Modifiers::default());
});
