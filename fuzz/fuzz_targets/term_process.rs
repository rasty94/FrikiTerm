#![no_main]
use friki_term::{Key, Modifiers, Terminal};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // Primer byte: punto de corte (el estado debe sobrevivir entre llamadas).
    let (cut, rest) = data.split_first().map_or((0, data), |(c, r)| (*c as usize, r));
    let cut = cut.min(rest.len());
    let mut t = Terminal::new(24, 80, 100);
    t.process(&rest[..cut]);
    t.resize(10, 20);
    t.process(&rest[cut..]);
    let _ = t.screen();
    let s = String::from_utf8_lossy(rest);
    let needle: String = s.chars().take(4).collect();
    let _ = t.search(&needle, false);
    let _ = t.encode_paste(&s);
    let _ = t.encode_key(Key::Char('a'), Modifiers::default());
    t.scroll_view(-5);
});
