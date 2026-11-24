# Cambios de contrato pendientes del integrador

## Fase 5: exponer `Cli` para generar man y completions (`xtask`)

**Estado:** `xtask` compila y está en `members`, pero `friki_command()` devuelve error hasta que
se exponga la definición de la CLI. Mientras tanto `cargo run -p xtask -- assets` falla con un
mensaje que apunta a este fichero, y `release.yml` (paso "Generar página man y completions") también.

**Causa:** `struct Cli` y `enum Command` viven privados en `crates/friki-cli/src/main.rs`. `xtask`
no puede importarlos sin copiar la definición (drift garantizado) ni incluir `main.rs` como módulo
(arrastraría todo el binario). La fase 5 no debe tocar `crates/*/src`.

**Propuesta:**

1. Mover `Cli` y `Command` (y los tipos de sus argumentos) de `crates/friki-cli/src/main.rs` a
   `crates/friki-cli/src/cli.rs`, marcados `pub`. `main.rs` pasa a `use friki_cli::cli::Cli;` (o
   `mod cli; use cli::Cli;` si no se quiere lib).
2. Añadir `crates/friki-cli/src/lib.rs` con `pub mod cli;` y, en `crates/friki-cli/Cargo.toml`,
   una sección `[lib]` (`path = "src/lib.rs"`). Un paquete con lib y bin conviven sin problema.
3. En `xtask/Cargo.toml` añadir `friki-cli = { path = "../crates/friki-cli" }`.
4. En `xtask/src/main.rs` sustituir el cuerpo de `friki_command()` por:
   ```rust
   use clap::CommandFactory;
   Ok(friki_cli::cli::Cli::command())
   ```
   (y quitar el `Err` de placeholder).

**Efecto:** `cargo run -p xtask -- assets` escribe `crates/friki-cli/assets/gen/friki.1` y
`completions/{friki.bash,_friki,friki.fish,_friki.ps1}`, que consumen `[package.metadata.deb]` y
`release.yml`.

## Otros puntos para el integrador (fuera de la sección `[package.metadata.deb]`)

- `crates/friki-cli/Cargo.toml` no tiene `description` en `[package]`: `cargo deb` avisa y la
  descripción del control queda vacía. Añadir `description = "..."` en `[package]`.
- `[package.metadata.deb].maintainer` es un marcador (`frikiSSH <noreply@invalid>`) hasta fijar
  el mantenedor real. Es obligatorio para el `.deb`.
- `LICENSE` sigue sin existir (TODO §0). Cuando exista, añadir `license-file` en
  `[package.metadata.deb]`.
- El workflow de release usa `ubuntu-24.04-arm` y `macos-15-intel` como etiquetas de runner.
  Confirmar que el repositorio tiene acceso a esos runners (en repos privados requieren plan
  adecuado).
