use russh::Error as R;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Resolución, conexión, timeout o corte de red.
    #[error("red: {0}")]
    Network(String),
    #[error("autenticación: {0}")]
    Auth(String),
    #[error("clave de host rechazada: {0}")]
    HostKey(String),
    /// Sin algoritmos comunes con el servidor.
    #[error("negociación de algoritmos: {0}")]
    Negotiation(String),
    #[error("{0}")]
    Other(String),
}

/// Clasifica un error de russh. `host_rejected`: el verificador devolvió `Reject`
/// (russh lo reporta como un error genérico de clave desconocida).
pub(crate) fn classify(e: R, host_rejected: bool) -> Error {
    if host_rejected {
        return Error::HostKey("el verificador rechazó la clave del servidor".into());
    }
    let msg = e.to_string();
    match e {
        R::NoCommonAlgo { .. } | R::UnknownAlgo => Error::Negotiation(msg),
        R::KeyChanged { .. } | R::UnknownKey | R::WrongServerSig => Error::HostKey(msg),
        R::NotAuthenticated | R::NoAuthMethod | R::UnsupportedAuthMethod => Error::Auth(msg),
        R::IO(_)
        | R::HUP
        | R::Disconnect
        | R::ConnectionTimeout
        | R::KeepaliveTimeout
        | R::InactivityTimeout
        | R::Elapsed(_)
        | R::SendError
        | R::RecvError => Error::Network(msg),
        _ => Error::Other(msg),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clasifica_errores() {
        let c = |e| classify(e, false);
        let no_common = R::NoCommonAlgo {
            kind: russh::AlgorithmKind::Kex,
            ours: vec![],
            theirs: vec![],
        };
        assert!(matches!(c(no_common), Error::Negotiation(_)));
        assert!(matches!(c(R::HUP), Error::Network(_)));
        assert!(matches!(c(R::ConnectionTimeout), Error::Network(_)));
        assert!(matches!(
            c(R::IO(std::io::ErrorKind::ConnectionRefused.into())),
            Error::Network(_)
        ));
        assert!(matches!(c(R::KeyChanged { line: 1 }), Error::HostKey(_)));
        assert!(matches!(c(R::NotAuthenticated), Error::Auth(_)));
        assert!(matches!(c(R::Kex), Error::Other(_)));
        // Un rechazo del verificador gana sobre el error genérico de russh.
        assert!(matches!(classify(R::UnknownKey, true), Error::HostKey(_)));
        assert!(matches!(classify(R::Disconnect, true), Error::HostKey(_)));
    }
}
