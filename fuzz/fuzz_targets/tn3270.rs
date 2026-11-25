#![no_main]
use friki_tn3270::screen::Screen;
use friki_tn3270::telnet::Telnet;
use friki_tn3270::{Client, Tn3270Options};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // Primer byte: punto de corte; segundo: modelo/TN3270E. El estado sobrevive entre llamadas.
    let (head, rest) = data.split_at(data.len().min(2));
    let cut = usize::from(head.first().copied().unwrap_or(0)).min(rest.len());
    let model = 2 + head.get(1).copied().unwrap_or(0) % 4;
    let tn3270e = model % 2 == 0;

    // Negociación Telnet y registros de datos 3270 que produce.
    let mut t = Telnet::new("IBM-3278-2-E", tn3270e);
    let (mut reply, mut records) = (Vec::new(), Vec::new());
    t.feed(&rest[..cut], &mut reply, &mut records);
    t.feed(&rest[cut..], &mut reply, &mut records);
    let _ = t.encode_record(rest);

    // Flujo de datos 3270 sobre los registros recibidos.
    let mut scr = Screen::model(model);
    for rec in &records {
        let _ = scr.process(rec);
    }
    // Cliente completo (telnet + datastream) con el mismo flujo bruto.
    let mut c = Client::new(&Tn3270Options {
        model,
        tn3270e,
        connect_timeout_secs: 15,
    });
    let _ = c.feed(&rest[..cut]);
    let _ = c.feed(&rest[cut..]);
});
