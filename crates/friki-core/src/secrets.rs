//! Secretos en el llavero del sistema (Keychain, Secret Service, Credential Manager).
//! Nunca en texto plano ni con cifrado propio (TODO.md §8).

use crate::Result;

pub const SERVICE: &str = "frikiSSH";

pub fn get(account: &str) -> Result<Option<String>> {
    let _ = account;
    todo!("friki-core: secrets::get")
}

pub fn set(account: &str, secret: &str) -> Result<()> {
    let _ = (account, secret);
    todo!("friki-core: secrets::set")
}

pub fn delete(account: &str) -> Result<()> {
    let _ = account;
    todo!("friki-core: secrets::delete")
}
