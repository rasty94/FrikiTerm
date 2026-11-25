//! GSSAPI/Kerberos (`gssapi-with-mic`, RFC 4462). Feature `gssapi`, solo Unix (libgssapi del sistema).
//!
//! Las llamadas GSS son síncronas (el cliente Kerberos puede contactar con el KDC); se hacen
//! en línea. En Windows haría falta SSPI, que libgssapi no cubre.

use libgssapi::context::{ClientCtx, CtxFlags, SecurityContext};
use libgssapi::name::Name;
use libgssapi::oid::{GSS_MECH_KRB5, GSS_NT_HOSTBASED_SERVICE};
use russh::{GssapiAuthenticator, GssapiError, GssapiStep};

#[derive(Debug, thiserror::Error)]
pub(crate) enum GssError {
    #[error(transparent)]
    Send(#[from] russh::SendError),
    #[error("GSSAPI: {0}")]
    Gss(String),
}

fn gss<E: std::fmt::Display>(e: E) -> GssError {
    GssError::Gss(e.to_string())
}

pub(crate) struct Gssapi {
    ctx: ClientCtx,
    /// Último error del servidor (informativo).
    pub(crate) server_error: Option<String>,
}

/// OID de Kerberos 5 codificado en DER (tag 0x06 + longitud + contenido).
pub(crate) fn krb5_oid_der() -> Vec<u8> {
    let mut v = vec![0x06, GSS_MECH_KRB5.len() as u8];
    v.extend_from_slice(&GSS_MECH_KRB5);
    v
}

impl Gssapi {
    /// `host@<host>` es el principal de servicio. `delegate` reenvía el TGT (off por defecto).
    pub(crate) fn new(host: &str, delegate: bool) -> Result<Self, String> {
        let target = Name::new(
            format!("host@{host}").as_bytes(),
            Some(GSS_NT_HOSTBASED_SERVICE),
        )
        .map_err(|e| e.to_string())?;
        let mut flags = CtxFlags::GSS_C_MUTUAL_FLAG | CtxFlags::GSS_C_INTEG_FLAG;
        if delegate {
            flags |= CtxFlags::GSS_C_DELEG_FLAG;
        }
        Ok(Self {
            ctx: ClientCtx::new(None, target, flags, Some(GSS_MECH_KRB5)),
            server_error: None,
        })
    }
}

impl GssapiAuthenticator for Gssapi {
    type Error = GssError;

    async fn gssapi_step(
        &mut self,
        selected_mechanism: Option<Vec<u8>>,
        input_token: Option<Vec<u8>>,
        mic_data: Vec<u8>,
    ) -> Result<GssapiStep, GssError> {
        if selected_mechanism.is_some_and(|m| m != krb5_oid_der()) {
            return Err(gss("el servidor eligió un mecanismo que no se ofreció"));
        }
        let out = self
            .ctx
            .step(input_token.as_deref(), None)
            .map_err(gss)?
            .map(|b| b.to_vec());
        if self.ctx.is_complete() {
            let mic = self.ctx.get_mic(&mic_data).map_err(gss)?.to_vec();
            Ok(GssapiStep::Complete {
                token: out,
                mic: Some(mic),
            })
        } else {
            Ok(GssapiStep::Continue {
                token: out.unwrap_or_default(),
            })
        }
    }

    async fn gssapi_error(&mut self, error: GssapiError) {
        if let GssapiError::Status { message, .. } = error {
            self.server_error = Some(message);
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn oid_krb5_der() {
        assert_eq!(
            super::krb5_oid_der(),
            [0x06, 9, 0x2A, 0x86, 0x48, 0x86, 0xF7, 0x12, 1, 2, 2]
        );
    }
}
