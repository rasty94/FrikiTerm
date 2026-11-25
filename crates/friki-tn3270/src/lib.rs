//! Emulador TN3270 (IBM 3270 Data Stream) para FrikiTerm: negociación Telnet, búfer de
//! pantalla, órdenes de escritura y entrada del usuario. No pinta nada: la TUI usa `render`.

pub mod client;
pub mod datastream;
pub mod ebcdic;
pub mod keys;
pub mod render;
pub mod screen;
pub mod session;
pub mod telnet;

#[cfg(test)]
mod tests;

pub use client::{Client, Output, Tn3270Options};
pub use keys::Key;
pub use screen::{Cell, Screen};
