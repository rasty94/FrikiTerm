#![no_main]
use friki_telnet::proto::{Parser, Session};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let (cut, rest) = data.split_first().map_or((0, data), |(c, r)| (*c as usize, r));
    let cut = cut.min(rest.len());
    let (mut p, mut s) = (Parser::default(), Session::new("xterm", 80, 24));
    for chunk in [&rest[..cut], &rest[cut..]] {
        let _ = s.process(p.feed(chunk));
    }
    let _ = s.encode(rest);
    let _ = s.resize(1, 1);
});
