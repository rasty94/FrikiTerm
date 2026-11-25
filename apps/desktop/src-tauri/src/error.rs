//! Error común de todos los comandos: `{ kind, message }`.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ErrorKind {
    Auth,
    Hostkey,
    Network,
    Notfound,
    Denied,
    Invalid,
    Unsupported,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[error("{message}")]
pub struct CmdError {
    pub kind: ErrorKind,
    pub message: String,
}

impl CmdError {
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }
    pub fn invalid(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Invalid, message)
    }
    pub fn unsupported(what: &str) -> Self {
        Self::new(ErrorKind::Unsupported, format!("{what}: no implementado"))
    }
}

/// Atajo para los comandos aún sin implementar.
pub fn unsupported<T>(what: &str) -> Result<T, CmdError> {
    Err(CmdError::unsupported(what))
}

impl From<friki_ssh::Error> for CmdError {
    fn from(e: friki_ssh::Error) -> Self {
        use friki_ssh::Error as E;
        let kind = match &e {
            E::Auth(_) => ErrorKind::Auth,
            E::HostKey(_) => ErrorKind::Hostkey,
            E::Network(_) | E::Negotiation(_) => ErrorKind::Network,
            E::Other(_) => ErrorKind::Other,
        };
        Self::new(kind, e.to_string())
    }
}

impl From<friki_xfer::Error> for CmdError {
    fn from(e: friki_xfer::Error) -> Self {
        use friki_xfer::ErrorKind as K;
        let kind = match e.kind() {
            K::Network => ErrorKind::Network,
            K::NotFound => ErrorKind::Notfound,
            K::PermissionDenied => ErrorKind::Denied,
            K::Protocol | K::Cancelled | K::Other => ErrorKind::Other,
        };
        Self::new(kind, e.to_string())
    }
}

impl From<std::io::Error> for CmdError {
    fn from(e: std::io::Error) -> Self {
        use std::io::ErrorKind as K;
        let kind = match e.kind() {
            K::NotFound => ErrorKind::Notfound,
            K::PermissionDenied => ErrorKind::Denied,
            K::ConnectionReset
            | K::ConnectionAborted
            | K::ConnectionRefused
            | K::NotConnected
            | K::BrokenPipe
            | K::TimedOut
            | K::UnexpectedEof => ErrorKind::Network,
            _ => ErrorKind::Other,
        };
        Self::new(kind, e.to_string())
    }
}

impl From<friki_core::Error> for CmdError {
    fn from(e: friki_core::Error) -> Self {
        Self::new(ErrorKind::Other, e.to_string())
    }
}

impl From<anyhow::Error> for CmdError {
    fn from(e: anyhow::Error) -> Self {
        Self::new(ErrorKind::Other, format!("{e:#}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serializa_como_contrato() {
        let e = CmdError::from(friki_ssh::Error::HostKey("x".into()));
        assert_eq!(
            serde_json::to_string(&e).unwrap(),
            r#"{"kind":"hostkey","message":"clave de host rechazada: x"}"#
        );
        let e = CmdError::from(std::io::Error::from(std::io::ErrorKind::NotFound));
        assert_eq!(e.kind, ErrorKind::Notfound);
    }
}
