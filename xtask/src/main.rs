//! Tareas de desarrollo y empaquetado (no se publica).
//!
//! `cargo run -p xtask -- assets` genera la página man (`friki.1`) y las
//! completions de bash, zsh, fish y PowerShell en `crates/friki-cli/assets/gen/`.
//! Esos ficheros los consume `cargo deb` (ver `[package.metadata.deb]`).

use std::path::Path;
use std::process::ExitCode;

use clap::Command;
use clap_complete::Shell;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("xtask: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    match std::env::args().nth(1).as_deref() {
        Some("assets") => assets(),
        _ => Err("uso: cargo run -p xtask -- assets".into()),
    }
}

fn assets() -> Result<(), Box<dyn std::error::Error>> {
    let mut cmd = friki_command()?;
    let out = Path::new(env!("CARGO_MANIFEST_DIR")).join("../crates/friki-cli/assets/gen");
    let completions = out.join("completions");
    std::fs::create_dir_all(&completions)?;

    let mut man = Vec::new();
    clap_mangen::Man::new(cmd.clone()).render(&mut man)?;
    std::fs::write(out.join("friki.1"), man)?;

    for shell in [Shell::Bash, Shell::Zsh, Shell::Fish, Shell::PowerShell] {
        clap_complete::generate_to(shell, &mut cmd, "friki", &completions)?;
    }
    Ok(())
}

/// Devuelve el `clap::Command` de `friki`.
///
/// TODO(integrador): sustituir el cuerpo por `Ok(friki_cli::cli::Cli::command())`
/// (o la ruta que se acuerde). Hoy `Cli` es privado en `crates/friki-cli/src/main.rs`
/// y no hay forma de importarlo sin tocar `src/`. La propuesta para exponerlo
/// (mover `Cli` a `crates/friki-cli/src/cli.rs` y reexportarlo) está en
/// `CONTRACT-CHANGES.md` de la raíz del repositorio.
fn friki_command() -> Result<Command, String> {
    Err("definición de la CLI no expuesta todavía; ver CONTRACT-CHANGES.md".into())
}
