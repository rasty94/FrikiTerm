//! Banco de pruebas de emulación con *golden files*.
//!
//! Cada caso es un par en `tests/golden/`:
//! - `<nombre>.in`: cabecera `# size: FILAS COLS` y comentarios `#` con la fuente de la
//!   especificación. Cada línea restante es un trozo de entrada que se pasa en una
//!   llamada a `process` (así se prueban secuencias partidas entre trozos). Escapes:
//!   `\e` ESC, `\r`, `\n`, `\t`, `\a` BEL, `\b` BS, `\\`, `\xNN` byte, `\u{H}` code point
//!   UTF-8. Texto UTF-8 directo también vale. Una línea `@resize FILAS COLS` redimensiona.
//! - `<nombre>.out`: volcado esperado (ver `dump`).
//!
//! `FRIKI_GOLDEN_UPDATE=1 cargo test -p friki-term --test golden` reescribe los `.out`
//! con el comportamiento actual, salvo los casos de `KNOWN_DIFFERENCES`, cuyo `.out`
//! guarda lo correcto según la fuente. Revisa siempre el `git diff` de los `.out`.

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use friki_core::config::{Emulation, TerminalProfile};
use friki_term::{Cell, Color, Terminal};

/// Diferencias conocidas entre el emulador y la fuente citada en el `.in`. El `.out` del
/// caso guarda el comportamiento CORRECTO; el test exige que el actual difiera, así que
/// si el emulador se corrige hay que quitar la entrada.
const KNOWN_DIFFERENCES: &[(&str, &str)] = &[];

const SCROLLBACK: usize = 100;

enum Step {
    Bytes(Vec<u8>),
    Resize(u16, u16),
}

struct Case {
    size: (u16, u16),
    emulation: Emulation,
    steps: Vec<Step>,
}

fn unescape(line: &str) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            let mut buf = [0u8; 4];
            out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
            continue;
        }
        match chars.next() {
            Some('e') => out.push(0x1b),
            Some('r') => out.push(b'\r'),
            Some('n') => out.push(b'\n'),
            Some('t') => out.push(b'\t'),
            Some('a') => out.push(0x07),
            Some('b') => out.push(0x08),
            Some('\\') => out.push(b'\\'),
            Some('x') => {
                let hex: String = chars.by_ref().take(2).collect();
                let byte = u8::from_str_radix(&hex, 16)
                    .map_err(|_| format!("escape \\x{hex} inválido"))?;
                out.push(byte);
            }
            Some('u') => {
                if chars.next() != Some('{') {
                    return Err("\\u sin '{'".into());
                }
                let hex: String = chars.by_ref().take_while(|&c| c != '}').collect();
                let cp = u32::from_str_radix(&hex, 16)
                    .ok()
                    .and_then(char::from_u32)
                    .ok_or_else(|| format!("code point \\u{{{hex}}} inválido"))?;
                let mut buf = [0u8; 4];
                out.extend_from_slice(cp.encode_utf8(&mut buf).as_bytes());
            }
            other => return Err(format!("escape desconocido {other:?}")),
        }
    }
    Ok(out)
}

fn parse_case(text: &str) -> Result<Case, String> {
    let mut size = None;
    let mut emulation = Emulation::default();
    let mut steps = Vec::new();
    for (n, line) in text.lines().enumerate() {
        let n = n + 1;
        if let Some(rest) = line.strip_prefix("# size:") {
            let v: Vec<u16> = rest
                .split_whitespace()
                .map(str::parse)
                .collect::<Result<_, _>>()
                .map_err(|e| format!("línea {n}: {e}"))?;
            match v[..] {
                [r, c] => size = Some((r, c)),
                _ => return Err(format!("línea {n}: se esperan FILAS COLS")),
            }
        } else if let Some(rest) = line.strip_prefix("# emulation:") {
            emulation = match rest.trim() {
                "wyse50" => Emulation::Wyse50,
                "wyse60" => Emulation::Wyse60,
                "tvi910" => Emulation::Tvi910,
                "tvi925" => Emulation::Tvi925,
                o => return Err(format!("línea {n}: emulación {o:?} desconocida")),
            };
        } else if line.starts_with('#') || line.is_empty() {
            continue;
        } else if let Some(rest) = line.strip_prefix("@resize ") {
            let v: Vec<u16> = rest
                .split_whitespace()
                .map(str::parse)
                .collect::<Result<_, _>>()
                .map_err(|e| format!("línea {n}: {e}"))?;
            match v[..] {
                [r, c] => steps.push(Step::Resize(r, c)),
                _ => return Err(format!("línea {n}: @resize necesita FILAS COLS")),
            }
        } else {
            steps.push(Step::Bytes(
                unescape(line).map_err(|e| format!("línea {n}: {e}"))?,
            ));
        }
    }
    let size = size.ok_or("falta la cabecera '# size: FILAS COLS'")?;
    Ok(Case {
        size,
        emulation,
        steps,
    })
}

fn color_str(c: Color) -> Option<String> {
    match c {
        Color::Default => None,
        Color::Indexed(n) => Some(format!("idx{n}")),
        Color::Rgb(r, g, b) => Some(format!("#{r:02x}{g:02x}{b:02x}")),
    }
}

fn attr_parts(c: &Cell) -> Vec<String> {
    let mut v = Vec::new();
    if c.bold {
        v.push("bold".to_owned());
    }
    if c.italic {
        v.push("italic".to_owned());
    }
    if c.underline {
        v.push("underline".to_owned());
    }
    if c.inverse {
        v.push("inverse".to_owned());
    }
    if let Some(fg) = color_str(c.fg) {
        v.push(format!("fg={fg}"));
    }
    if let Some(bg) = color_str(c.bg) {
        v.push(format!("bg={bg}"));
    }
    v
}

/// Formato del volcado:
/// - `size FILASxCOLS`, `cursor FILA,COL visible|oculto`, `alt sí|no`,
///   `scrollback N`, `title "..."`.
/// - `rows:` y una línea `NN|texto|` por fila. Cada celda ocupa un carácter; una celda
///   vacía (incluida la segunda mitad de un carácter ancho) se muestra como espacio, así
///   que un carácter ancho ocupa dos caracteres. Se quitan los espacios finales.
/// - `attrs:` con una línea `F,C: atributos` por celda con algún atributo no por defecto
///   (`bold italic underline inverse fg=... bg=...`), o `attrs: ninguno`.
fn dump(t: &Terminal) -> String {
    let s = t.screen();
    let mut o = String::new();
    let _ = writeln!(o, "size {}x{}", s.rows, s.cols);
    let vis = if s.cursor_visible {
        "visible"
    } else {
        "oculto"
    };
    let _ = writeln!(o, "cursor {},{} {vis}", s.cursor.0, s.cursor.1);
    let _ = writeln!(o, "alt {}", if t.alternate_screen() { "sí" } else { "no" });
    let _ = writeln!(o, "scrollback {}", t.scrollback_len());
    let _ = writeln!(o, "title {:?}", s.title);
    let _ = writeln!(o, "rows:");
    let cols = usize::from(s.cols);
    for r in 0..s.rows {
        let mut line = String::new();
        for c in 0..cols {
            let cell = &s.cells[usize::from(r) * cols + c];
            if cell.text.is_empty() {
                line.push(' ');
            } else {
                line.push_str(&cell.text);
            }
        }
        let _ = writeln!(o, "{r:02}|{}|", line.trim_end());
    }
    let mut attrs = String::new();
    for r in 0..s.rows {
        for c in 0..cols {
            let cell = &s.cells[usize::from(r) * cols + c];
            let parts = attr_parts(cell);
            if !parts.is_empty() {
                let _ = writeln!(attrs, "  {r},{c}: {}", parts.join(" "));
            }
        }
    }
    if attrs.is_empty() {
        let _ = writeln!(o, "attrs: ninguno");
    } else {
        let _ = writeln!(o, "attrs:");
        o.push_str(&attrs);
    }
    o
}

fn run(case: &Case) -> String {
    let (rows, cols) = case.size;
    let profile = TerminalProfile {
        scrollback: SCROLLBACK,
        emulation: case.emulation,
        ..TerminalProfile::default()
    };
    let mut t = Terminal::with_profile(rows, cols, &profile);
    for step in &case.steps {
        match step {
            Step::Bytes(b) => t.process(b),
            Step::Resize(r, c) => t.resize(*r, *c),
        }
    }
    dump(&t)
}

fn golden_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden")
}

#[test]
fn golden_files() {
    let update = std::env::var_os("FRIKI_GOLDEN_UPDATE").is_some();
    let mut inputs: Vec<PathBuf> = fs::read_dir(golden_dir())
        .expect("leer tests/golden")
        .map(|e| e.expect("entrada de directorio").path())
        .filter(|p| p.extension().is_some_and(|e| e == "in"))
        .collect();
    inputs.sort();
    assert!(
        inputs.len() >= 25,
        "se esperan al menos 25 casos, hay {}",
        inputs.len()
    );

    for (name, _) in KNOWN_DIFFERENCES {
        assert!(
            golden_dir().join(format!("{name}.in")).exists(),
            "KNOWN_DIFFERENCES cita un caso que no existe: {name}"
        );
    }

    let mut failures = Vec::new();
    for input in &inputs {
        let name = input
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("?")
            .to_owned();
        let actual =
            match parse_case(&fs::read_to_string(input).expect("leer .in")).map(|c| run(&c)) {
                Ok(a) => a,
                Err(e) => {
                    failures.push(format!("{name}: caso mal formado: {e}"));
                    continue;
                }
            };
        let out_path = input.with_extension("out");
        let known = KNOWN_DIFFERENCES.iter().find(|(n, _)| *n == name);

        if let Some((_, why)) = known {
            let expected = fs::read_to_string(&out_path).unwrap_or_default();
            if expected == actual {
                failures.push(format!(
                    "{name}: está en KNOWN_DIFFERENCES pero ya coincide con la fuente; quítalo"
                ));
            } else if !update {
                // Diferencia conocida: el test sigue ahí, no rompe el CI.
                eprintln!("golden {name}: diferencia conocida ({why})");
            }
            continue;
        }

        if update {
            fs::write(&out_path, &actual).expect("escribir .out");
            continue;
        }
        match fs::read_to_string(&out_path) {
            Ok(expected) if expected == actual => {}
            Ok(expected) => failures.push(format!(
                "{name}: el volcado no coincide\n--- esperado\n{expected}--- obtenido\n{actual}"
            )),
            Err(_) => failures.push(format!("{name}: falta {}", out_path.display())),
        }
    }

    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}
