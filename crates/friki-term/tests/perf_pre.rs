//! Coste del preprocesador de bytes frente a `vt100` a pelo, en un volcado grande
//! (texto con SGR, UTF-8 y algún tabulador). Solo en release:
//! `cargo test -p friki-term --release --test perf_pre -- --ignored --nocapture`

use std::num::NonZeroU16;
use std::time::{Duration, Instant};

use friki_term::Terminal;

fn dump() -> Vec<u8> {
    let mut v = Vec::new();
    for i in 0..200_000u32 {
        v.extend_from_slice(
            format!(
                "\x1b[32mdrwxr-xr-x\x1b[0m  {i:6} usuario  fichero_{i}.txt  ñandú año \u{2500}\u{2502}\t{}\r\n",
                "lorem ipsum dolor sit amet, consectetur adipiscing elit"
            )
            .as_bytes(),
        );
    }
    v
}

fn best<F: FnMut()>(mut f: F) -> Duration {
    (0..5)
        .map(|_| {
            let t = Instant::now();
            f();
            t.elapsed()
        })
        .min()
        .unwrap_or_default()
}

#[test]
#[ignore = "medición: solo en release"]
fn preprocesador_cuesta_menos_de_un_10_por_ciento() {
    let data = dump();
    let nz = |n| NonZeroU16::new(n).unwrap();
    let raw = best(|| {
        let mut p = vt100::Parser::new_with_callbacks(nz(50), nz(200), 1000, ());
        for c in data.chunks(16 * 1024) {
            p.process(c);
        }
    });
    let con = best(|| {
        let mut t = Terminal::new(50, 200, 1000);
        for c in data.chunks(16 * 1024) {
            t.process(c);
        }
    });
    let ratio = con.as_secs_f64() / raw.as_secs_f64();
    eprintln!(
        "{} MiB: vt100 {raw:?}, con preprocesador {con:?}, relación {ratio:.3}",
        data.len() >> 20
    );
    // Medido 1,06-1,09x en reposo; margen para máquinas ocupadas (1,14x observado).
    assert!(ratio < 1.20, "el preprocesador cuesta {ratio:.3}x");
}
