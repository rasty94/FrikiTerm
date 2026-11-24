//! Traducción de `AlgorithmPolicy` a `russh::Preferred`.

use std::borrow::Cow;

use russh::keys::{Algorithm, EcdsaCurve, HashAlg};
use russh::{Preferred, cipher, compression, kex, mac};

use crate::AlgorithmPolicy;

fn host_key_algorithm(name: &str) -> Option<Algorithm> {
    Some(match name {
        "ssh-ed25519" => Algorithm::Ed25519,
        "ecdsa-sha2-nistp256" => Algorithm::Ecdsa {
            curve: EcdsaCurve::NistP256,
        },
        "ecdsa-sha2-nistp384" => Algorithm::Ecdsa {
            curve: EcdsaCurve::NistP384,
        },
        "ecdsa-sha2-nistp521" => Algorithm::Ecdsa {
            curve: EcdsaCurve::NistP521,
        },
        "rsa-sha2-512" => Algorithm::Rsa {
            hash: Some(HashAlg::Sha512),
        },
        "rsa-sha2-256" => Algorithm::Rsa {
            hash: Some(HashAlg::Sha256),
        },
        "ssh-rsa" => Algorithm::Rsa { hash: None },
        "ssh-dss" => Algorithm::Dsa,
        _ => return None,
    })
}

fn conv<T>(
    names: &[&'static str],
    skipped: &mut Vec<&'static str>,
    parse: impl Fn(&'static str) -> Option<T>,
) -> Vec<T> {
    let mut out = Vec::new();
    for n in names {
        match parse(n) {
            Some(v) => out.push(v),
            None => skipped.push(*n),
        }
    }
    out
}

impl AlgorithmPolicy {
    /// Pone primero los algoritmos de clave de host que ya figuran en `known_hosts`
    /// para este host (como OpenSSH), conservando el orden relativo. Los
    /// `rsa-sha2-*` y `ssh-rsa` cuentan como la familia `ssh-rsa`.
    pub(crate) fn prefer_known(&mut self, known: &[String]) {
        let family = |n: &str| {
            if n.starts_with("rsa-sha2-") {
                "ssh-rsa"
            } else {
                n
            }
            .to_string()
        };
        self.host_keys.sort_by_key(|n| !known.contains(&family(n)));
    }

    /// Devuelve la configuración de russh y los nombres que russh no conoce
    /// (omitidos). Añade las extensiones de strict KEX (CVE-2023-48795).
    pub(crate) fn to_preferred(&self) -> (Preferred, Vec<&'static str>) {
        let mut skipped = Vec::new();
        let mut kexes = conv(&self.kex, &mut skipped, |n| kex::Name::try_from(n).ok());
        // Strict KEX necesita anunciarse en la lista de KEX del cliente.
        kexes.push(kex::EXTENSION_SUPPORT_AS_CLIENT);
        kexes.push(kex::EXTENSION_OPENSSH_STRICT_KEX_AS_CLIENT);
        let ciphers = conv(&self.ciphers, &mut skipped, |n| {
            cipher::Name::try_from(n).ok()
        });
        let macs = conv(&self.macs, &mut skipped, |n| mac::Name::try_from(n).ok());
        let keys = conv(&self.host_keys, &mut skipped, host_key_algorithm);

        let preferred = Preferred {
            kex: Cow::Owned(kexes),
            key: Cow::Owned(keys),
            cipher: Cow::Owned(ciphers),
            mac: Cow::Owned(macs),
            // Sin compresión: ni `zlib` ni `zlib@openssh.com` (superficie y CRIME-style).
            compression: Cow::Borrowed(&[compression::NONE]),
            ..Preferred::default()
        };
        (preferred, skipped)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEBILES: &[&str] = &[
        "3des", "cbc", "sha1", "md5", "rc4", "group1-", "ssh-dss", "ssh-rsa", "arcfour",
    ];

    fn all(p: &AlgorithmPolicy) -> Vec<&'static str> {
        [&p.kex, &p.ciphers, &p.macs, &p.host_keys]
            .into_iter()
            .flatten()
            .copied()
            .collect()
    }

    #[test]
    fn secure_no_tiene_nada_debil() {
        for n in all(&AlgorithmPolicy::secure()) {
            for d in DEBILES {
                assert!(!n.contains(d), "{n} contiene {d}");
            }
        }
    }

    #[test]
    fn secure_ofrecido_a_russh_no_tiene_nada_debil() {
        let (p, _) = AlgorithmPolicy::secure().to_preferred();
        let ofrecidos: Vec<String> = p
            .kex
            .iter()
            .map(|n| n.as_ref().to_string())
            .chain(p.cipher.iter().map(|n| n.as_ref().to_string()))
            .chain(p.mac.iter().map(|n| n.as_ref().to_string()))
            .chain(p.key.iter().map(|n| n.to_string()))
            .collect();
        assert!(!ofrecidos.is_empty());
        for n in &ofrecidos {
            for d in ["none", "-cbc", "sha1", "md5", "group1-", "dss"] {
                assert!(!n.contains(d), "{n} contiene {d}");
            }
        }
        assert_eq!(p.compression.as_ref(), [compression::NONE]);
    }

    #[test]
    fn conocidos_van_primero_y_el_resto_conserva_orden() {
        let mut p = AlgorithmPolicy::secure();
        p.prefer_known(&["ecdsa-sha2-nistp256".into(), "ssh-rsa".into()]);
        assert_eq!(
            p.host_keys,
            [
                "ecdsa-sha2-nistp256",
                "rsa-sha2-512",
                "rsa-sha2-256",
                "ssh-ed25519",
                "ecdsa-sha2-nistp384",
                "ecdsa-sha2-nistp521"
            ]
        );
        let mut q = AlgorithmPolicy::secure();
        q.prefer_known(&[]);
        assert_eq!(q, AlgorithmPolicy::secure());
    }

    #[test]
    fn secure_la_conoce_russh_entera() {
        let (pref, skipped) = AlgorithmPolicy::secure().to_preferred();
        assert!(skipped.is_empty(), "russh no conoce: {skipped:?}");
        // 9 KEX + ext-info-c + strict KEX
        assert_eq!(pref.kex.len(), 11);
        assert!(
            pref.kex
                .contains(&kex::EXTENSION_OPENSSH_STRICT_KEX_AS_CLIENT)
        );
        assert_eq!(pref.cipher.len(), 6);
        assert_eq!(pref.mac.len(), 4);
        assert_eq!(pref.key.len(), 6);
    }

    #[test]
    fn nombre_desconocido_se_omite() {
        let mut p = AlgorithmPolicy::secure();
        p.kex.push("kex-inventado");
        p.ciphers.push("cifrado-inventado");
        p.macs.push("mac-inventado");
        p.host_keys.push("clave-inventada");
        let (pref, skipped) = p.to_preferred();
        assert_eq!(
            skipped,
            [
                "kex-inventado",
                "cifrado-inventado",
                "mac-inventado",
                "clave-inventada"
            ]
        );
        assert_eq!(pref.cipher.len(), 6);
        assert_eq!(pref.key.len(), 6);
    }

    #[test]
    fn for_session_sin_legacy_es_secure() {
        let cfg = friki_core::config::SessionConfig {
            legacy_algorithms: false,
            ..Default::default()
        };
        assert_eq!(
            AlgorithmPolicy::for_session(&cfg),
            AlgorithmPolicy::secure()
        );
    }

    #[cfg(feature = "legacy")]
    #[test]
    fn legacy_la_conoce_russh_y_empieza_por_secure() {
        let legacy = AlgorithmPolicy::legacy();
        let (_, skipped) = legacy.to_preferred();
        assert!(skipped.is_empty(), "russh no conoce: {skipped:?}");
        let secure = AlgorithmPolicy::secure();
        assert_eq!(legacy.kex[..secure.kex.len()], secure.kex[..]);
        assert!(legacy.ciphers.contains(&"3des-cbc") && legacy.host_keys.contains(&"ssh-dss"));
        let cfg = friki_core::config::SessionConfig {
            legacy_algorithms: true,
            ..Default::default()
        };
        assert_eq!(AlgorithmPolicy::for_session(&cfg), legacy);
    }
}
