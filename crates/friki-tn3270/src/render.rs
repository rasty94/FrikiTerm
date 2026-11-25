//! Pantalla 3270 -> secuencias ANSI para el emulador del panel (redibujo completo).

use std::fmt::Write;

use crate::ebcdic;
use crate::screen::{DISP_HIDDEN, DISP_INTENSE, FA_DISPLAY, FA_PROTECTED, Screen};

/// SGR de una celda a partir del campo que la gobierna y los atributos extendidos.
fn sgr(fa: u8, hl: u8, fg: u8) -> String {
    let prot = fa & FA_PROTECTED != 0;
    let intense = fa & FA_DISPLAY == DISP_INTENSE;
    // colores clásicos: entrada verde, entrada intensa rojo, protegido azul, protegido intenso blanco
    let mut color = match (prot, intense) {
        (false, false) => 32,
        (false, true) => 31,
        (true, false) => 34,
        (true, true) => 37,
    };
    if (0xF1..=0xF7).contains(&fg) {
        color = [34, 31, 35, 32, 36, 33, 37][usize::from(fg - 0xF1)];
    }
    let mut s = format!("0;{color}");
    if intense {
        s.push_str(";1");
    }
    match hl {
        0xF1 => s.push_str(";5"),
        0xF2 => s.push_str(";7"),
        0xF4 => s.push_str(";4"),
        _ => {}
    }
    s
}

pub fn ansi(screen: &Screen) -> Vec<u8> {
    let (rows, cols) = (screen.rows(), screen.cols());
    let mut out = String::from("\x1b[?25l");
    // campo vigente al empezar: el de la última marca del búfer (vuelta)
    let mut fa = screen.field_at(rows * cols - 1).map_or(0, |(_, f)| f);
    let mut last = String::new();
    for r in 0..rows {
        let _ = write!(out, "\x1b[{};1H", r + 1);
        for c in 0..cols {
            let Some(cell) = screen.cell(r, c) else {
                continue;
            };
            let ch = match cell.fa {
                Some(f) => {
                    fa = f;
                    ' '
                }
                None if fa & FA_DISPLAY == DISP_HIDDEN => ' ',
                None if cell.ext.cs == 0xF1 => '¤',
                None => ebcdic::to_char(cell.ch),
            };
            let s = sgr(fa, cell.ext.hl, cell.ext.fg);
            if s != last {
                let _ = write!(out, "\x1b[{s}m");
                last = s;
            }
            out.push(ch);
        }
    }
    let (r, c) = screen.cursor_pos();
    let _ = write!(out, "\x1b[0m\x1b[{};{}H\x1b[?25h", r + 1, c + 1);
    out.into_bytes()
}
