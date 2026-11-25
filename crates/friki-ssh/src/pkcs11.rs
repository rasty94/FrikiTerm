//! Claves en smartcard/HSM por PKCS#11 (feature `pkcs11`).
//!
//! La clave privada nunca sale del token: solo se listan las públicas y se firma con
//! `C_Sign`. Soporta RSA (rsa-sha2-256/512) y ECDSA P-256/384/521. No Ed25519 ni
//! `ssh-rsa` (SHA-1). Las llamadas PKCS#11 son síncronas y breves; se hacen en línea.

use cryptoki::context::{CInitializeArgs, CInitializeFlags, Pkcs11};
use cryptoki::mechanism::Mechanism;
use cryptoki::object::{Attribute, AttributeType, KeyType, ObjectClass, ObjectHandle};
use cryptoki::session::{Session, UserType};
use cryptoki::slot::Slot;
use cryptoki::types::AuthPin;
use russh::keys::agent::AgentIdentity;
use russh::keys::ssh_key::public::{EcdsaPublicKey, KeyData, RsaPublicKey};
use russh::keys::ssh_key::{Algorithm, EcdsaCurve, Mpint};
use russh::keys::{HashAlg, PublicKey};
use sha2::{Digest, Sha256, Sha384, Sha512};
use std::path::Path;

#[derive(Debug, thiserror::Error)]
pub(crate) enum SignError {
    #[error(transparent)]
    Send(#[from] russh::SendError),
    #[error("PKCS#11: {0}")]
    Token(String),
}

fn tok<E: std::fmt::Display>(e: E) -> SignError {
    SignError::Token(e.to_string())
}

/// Módulo cargado y sus tokens.
pub(crate) struct Module(Pkcs11);

/// Único módulo del proceso. Nunca se descarga: softhsm2 2.7 se cae (SIGSEGV) al
/// cerrarse y C_Initialize no puede repetirse. Ponytail: un módulo por proceso.
static MODULE: std::sync::Mutex<Option<(std::path::PathBuf, &'static Module)>> =
    std::sync::Mutex::new(None);

pub(crate) struct TokenSlot {
    pub slot: Slot,
    pub label: String,
    /// El token exige login para ver sus claves privadas.
    pub login: bool,
    /// Hay que pedir PIN (no hay lector con teclado propio).
    pub needs_pin: bool,
}

impl Module {
    /// Carga e inicializa el módulo la primera vez; luego devuelve el mismo.
    pub(crate) fn load(path: &Path) -> Result<&'static Self, String> {
        let mut cur = MODULE
            .lock()
            .map_err(|_| "estado PKCS#11 envenenado".to_owned())?;
        if let Some((p, m)) = cur.as_ref() {
            return if p == path {
                Ok(m)
            } else {
                Err("solo se admite un módulo PKCS#11 por proceso".into())
            };
        }
        let ctx = Pkcs11::new(path).map_err(|e| e.to_string())?;
        ctx.initialize(CInitializeArgs::new(CInitializeFlags::OS_LOCKING_OK))
            .map_err(|e| e.to_string())?;
        let m: &'static Self = Box::leak(Box::new(Self(ctx)));
        *cur = Some((path.to_path_buf(), m));
        Ok(m)
    }

    pub(crate) fn tokens(&self) -> Vec<TokenSlot> {
        let Ok(slots) = self.0.get_slots_with_token() else {
            return Vec::new();
        };
        slots
            .into_iter()
            .filter_map(|slot| {
                let t = self.0.get_token_info(slot).ok()?;
                Some(TokenSlot {
                    slot,
                    label: t.label().trim().to_owned(),
                    login: t.login_required(),
                    needs_pin: t.login_required() && !t.protected_authentication_path(),
                })
            })
            .collect()
    }

    /// Abre sesión de solo lectura e inicia sesión de usuario si hace falta (`pin` = `None`
    /// con PIN-pad del lector). Un PIN erróneo no se reintenta: gastaría intentos del token.
    pub(crate) fn open(&self, t: &TokenSlot, pin: Option<&str>) -> Result<Pkcs11Signer, String> {
        let session = self.0.open_ro_session(t.slot).map_err(|e| e.to_string())?;
        if t.login {
            let pin = pin.map(|p| AuthPin::from(p.to_owned()));
            session
                .login(UserType::User, pin.as_ref())
                .map_err(|e| e.to_string())?;
        }
        let keys = list_keys(&session).map_err(|e| e.to_string())?;
        Ok(Pkcs11Signer { session, keys })
    }
}

pub(crate) struct Pkcs11Signer {
    session: Session,
    keys: Vec<(PublicKey, ObjectHandle)>,
}

impl Pkcs11Signer {
    pub(crate) fn public_keys(&self) -> Vec<PublicKey> {
        self.keys.iter().map(|(k, _)| k.clone()).collect()
    }
}

fn list_keys(s: &Session) -> cryptoki::error::Result<Vec<(PublicKey, ObjectHandle)>> {
    let mut out = Vec::new();
    for h in s.find_objects(&[
        Attribute::Class(ObjectClass::PRIVATE_KEY),
        Attribute::Sign(true),
    ])? {
        let id = s
            .get_attributes(h, &[AttributeType::Id])?
            .into_iter()
            .find_map(|a| {
                if let Attribute::Id(v) = a {
                    Some(v)
                } else {
                    None
                }
            });
        // La pública suele ser otro objeto con el mismo CKA_ID; RSA a veces la lleva en la privada.
        let mut cands = match id {
            Some(id) => {
                s.find_objects(&[Attribute::Class(ObjectClass::PUBLIC_KEY), Attribute::Id(id)])?
            }
            None => Vec::new(),
        };
        cands.push(h);
        let key = cands.into_iter().find_map(|c| {
            let attrs = s
                .get_attributes(
                    c,
                    &[
                        AttributeType::KeyType,
                        AttributeType::Modulus,
                        AttributeType::PublicExponent,
                        AttributeType::EcPoint,
                        AttributeType::EcParams,
                    ],
                )
                .ok()?;
            public_from_attrs(&attrs)
        });
        if let Some(k) = key {
            out.push((k, h));
        }
    }
    Ok(out)
}

fn public_from_attrs(attrs: &[Attribute]) -> Option<PublicKey> {
    let (mut kt, mut n, mut e, mut pt, mut params) = (None, None, None, None, None);
    for a in attrs {
        match a {
            Attribute::KeyType(k) => kt = Some(*k),
            Attribute::Modulus(v) => n = Some(v.as_slice()),
            Attribute::PublicExponent(v) => e = Some(v.as_slice()),
            Attribute::EcPoint(v) => pt = Some(v.as_slice()),
            Attribute::EcParams(v) => params = Some(v.as_slice()),
            _ => {}
        }
    }
    let data: KeyData = match kt? {
        k if k == KeyType::RSA => RsaPublicKey::new(
            Mpint::from_positive_bytes(e?),
            Mpint::from_positive_bytes(n?),
        )
        .ok()?
        .into(),
        k if k == KeyType::EC => {
            let curve = curve_from_params(params?)?;
            let point = der_octet_string(pt?)?;
            let key = EcdsaPublicKey::from_sec1_bytes(point).ok()?;
            (key.curve() == curve).then_some(key)?.into()
        }
        _ => return None,
    };
    Some(PublicKey::from(data))
}

/// OID DER de la curva (`CKA_EC_PARAMS`).
fn curve_from_params(p: &[u8]) -> Option<EcdsaCurve> {
    match p {
        [0x06, 0x08, 0x2A, 0x86, 0x48, 0xCE, 0x3D, 0x03, 0x01, 0x07] => Some(EcdsaCurve::NistP256),
        [0x06, 0x05, 0x2B, 0x81, 0x04, 0x00, 0x22] => Some(EcdsaCurve::NistP384),
        [0x06, 0x05, 0x2B, 0x81, 0x04, 0x00, 0x23] => Some(EcdsaCurve::NistP521),
        _ => None,
    }
}

/// `CKA_EC_POINT` es el punto SEC1 envuelto en un OCTET STRING DER; algunos módulos lo dan sin envolver.
fn der_octet_string(b: &[u8]) -> Option<&[u8]> {
    let [tag, rest @ ..] = b else { return None };
    if *tag != 0x04 {
        return Some(b);
    }
    let (len, body) = match rest {
        [l, body @ ..] if *l < 0x80 => (*l as usize, body),
        [0x81, l, body @ ..] => (*l as usize, body),
        [0x82, h, l, body @ ..] => (usize::from(*h) << 8 | usize::from(*l), body),
        _ => return None,
    };
    // Un punto sin envolver también empieza por 0x04: se distingue por la longitud.
    if body.len() == len {
        Some(body)
    } else {
        Some(b)
    }
}

fn put(out: &mut Vec<u8>, b: &[u8]) {
    out.extend((b.len() as u32).to_be_bytes());
    out.extend(b);
}

/// Entero SSH (`mpint`) desde bytes sin signo big-endian.
fn put_mpint(out: &mut Vec<u8>, b: &[u8]) {
    let b = &b[b.iter().position(|&x| x != 0).unwrap_or(b.len())..];
    let pad = b.first().is_some_and(|x| x & 0x80 != 0);
    out.extend((b.len() as u32 + u32::from(pad)).to_be_bytes());
    if pad {
        out.push(0);
    }
    out.extend(b);
}

/// `r||s` crudo de PKCS#11 al blob de firma ECDSA de SSH.
fn ecdsa_blob(raw: &[u8]) -> Option<Vec<u8>> {
    if raw.is_empty() || !raw.len().is_multiple_of(2) {
        return None;
    }
    let (r, s) = raw.split_at(raw.len() / 2);
    let mut blob = Vec::new();
    put_mpint(&mut blob, r);
    put_mpint(&mut blob, s);
    Some(blob)
}

impl russh::Signer for Pkcs11Signer {
    type Error = SignError;

    fn auth_sign(
        &mut self,
        key: &AgentIdentity,
        hash_alg: Option<HashAlg>,
        mut to_sign: Vec<u8>,
    ) -> impl Future<Output = Result<Vec<u8>, SignError>> + Send {
        let r = self.sign(key, hash_alg, &to_sign).map(|(alg, sig)| {
            // Mismo formato que el agente: datos || string(string(alg) || string(firma)).
            let mut s = Vec::new();
            put(&mut s, alg.as_bytes());
            put(&mut s, &sig);
            put(&mut to_sign, &s);
            to_sign
        });
        async move { r }
    }
}

impl Pkcs11Signer {
    fn sign(
        &self,
        key: &AgentIdentity,
        hash_alg: Option<HashAlg>,
        data: &[u8],
    ) -> Result<(&'static str, Vec<u8>), SignError> {
        let AgentIdentity::PublicKey { key, .. } = key else {
            return Err(tok("los certificados no se firman con PKCS#11"));
        };
        let &(_, h) = self
            .keys
            .iter()
            .find(|(k, _)| k.key_data() == key.key_data())
            .ok_or_else(|| tok("la clave no está en el token"))?;
        match key.algorithm() {
            Algorithm::Rsa { .. } => {
                let (name, mech) = match hash_alg {
                    Some(HashAlg::Sha256) => ("rsa-sha2-256", Mechanism::Sha256RsaPkcs),
                    Some(HashAlg::Sha512) => ("rsa-sha2-512", Mechanism::Sha512RsaPkcs),
                    _ => return Err(tok("RSA solo con SHA-2")),
                };
                Ok((name, self.session.sign(&mech, h, data).map_err(tok)?))
            }
            Algorithm::Ecdsa { curve } => {
                let (name, digest) = match curve {
                    EcdsaCurve::NistP256 => ("ecdsa-sha2-nistp256", Sha256::digest(data).to_vec()),
                    EcdsaCurve::NistP384 => ("ecdsa-sha2-nistp384", Sha384::digest(data).to_vec()),
                    EcdsaCurve::NistP521 => ("ecdsa-sha2-nistp521", Sha512::digest(data).to_vec()),
                };
                let raw = self
                    .session
                    .sign(&Mechanism::Ecdsa, h, &digest)
                    .map_err(tok)?;
                Ok((
                    name,
                    ecdsa_blob(&raw).ok_or_else(|| tok("firma ECDSA inválida"))?,
                ))
            }
            _ => Err(tok("algoritmo de clave no soportado")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ec_point_envuelto_y_sin_envolver() {
        let mut p = vec![0x04];
        p.extend([7u8; 64]);
        let mut der = vec![0x04, 65];
        der.extend(&p);
        assert_eq!(der_octet_string(&der), Some(&p[..]));
        assert_eq!(der_octet_string(&p), Some(&p[..]));
    }

    #[test]
    fn mpint_quita_ceros_y_rellena_signo() {
        let mut o = Vec::new();
        put_mpint(&mut o, &[0, 0, 0x80, 1]);
        assert_eq!(o, [0, 0, 0, 3, 0, 0x80, 1]);
        let blob = ecdsa_blob(&[1, 2, 3, 4]).unwrap();
        assert_eq!(blob, [0, 0, 0, 2, 1, 2, 0, 0, 0, 2, 3, 4]);
        assert!(ecdsa_blob(&[1, 2, 3]).is_none());
    }

    #[test]
    fn curvas_conocidas() {
        assert_eq!(
            curve_from_params(&[0x06, 0x05, 0x2B, 0x81, 0x04, 0x00, 0x22]),
            Some(EcdsaCurve::NistP384)
        );
        assert_eq!(curve_from_params(&[0x06, 0x01, 0x00]), None);
    }

    /// Requiere softhsm2 con un token que tenga una clave privada con CKA_SIGN:
    /// `FRIKI_TEST_PKCS11_MODULE=/opt/homebrew/lib/softhsm/libsofthsm2.so`
    /// y opcionalmente `FRIKI_TEST_PKCS11_PIN` (si el token pide login).
    #[test]
    #[ignore = "requiere softhsm2 y FRIKI_TEST_PKCS11_MODULE"]
    fn softhsm_lista_y_firma() {
        let Ok(path) = std::env::var("FRIKI_TEST_PKCS11_MODULE") else {
            eprintln!("omitida: define FRIKI_TEST_PKCS11_MODULE");
            return;
        };
        let m = Module::load(Path::new(&path)).unwrap();
        let t = m.tokens().into_iter().next().expect("sin token");
        let pin = std::env::var("FRIKI_TEST_PKCS11_PIN").ok();
        let s = m.open(&t, pin.as_deref()).unwrap();
        let keys = s.public_keys();
        assert!(!keys.is_empty(), "el token no expone claves con CKA_SIGN");
        for k in keys {
            let hash = matches!(k.algorithm(), Algorithm::Rsa { .. }).then_some(HashAlg::Sha256);
            let id = AgentIdentity::PublicKey {
                key: k.clone(),
                comment: String::new(),
            };
            let (alg, sig) = s.sign(&id, hash, b"prueba").unwrap();
            assert!(!sig.is_empty(), "firma vacía con {alg}");
        }
    }
}
