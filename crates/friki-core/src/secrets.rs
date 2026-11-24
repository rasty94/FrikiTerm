//! Secretos en el llavero del sistema (Keychain, Secret Service, Credential Manager).
//! Nunca en texto plano ni con cifrado propio (TODO.md §8).

use crate::{Error, Result};

pub const SERVICE: &str = "frikiSSH";

fn entry(account: &str) -> Result<keyring::Entry> {
    keyring::Entry::new(SERVICE, account).map_err(|e| Error::Secret(e.to_string()))
}

pub fn get(account: &str) -> Result<Option<String>> {
    match entry(account)?.get_password() {
        Ok(s) => Ok(Some(s)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(Error::Secret(e.to_string())),
    }
}

pub fn set(account: &str, secret: &str) -> Result<()> {
    entry(account)?
        .set_password(secret)
        .map_err(|e| Error::Secret(e.to_string()))
}

/// Borrar un secreto inexistente no es un error.
pub fn delete(account: &str) -> Result<()> {
    match entry(account)?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(Error::Secret(e.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "toca el llavero real del sistema"]
    fn roundtrip_real_keyring() {
        if std::env::var("FRIKI_TEST_KEYRING").as_deref() != Ok("1") {
            eprintln!("omitida: define FRIKI_TEST_KEYRING=1 para usar el llavero real");
            return;
        }
        let acct = "friki-core-test-account";
        set(acct, "s3cr3t").unwrap();
        assert_eq!(get(acct).unwrap().as_deref(), Some("s3cr3t"));
        delete(acct).unwrap();
        assert_eq!(get(acct).unwrap(), None);
        delete(acct).unwrap();
    }
}
