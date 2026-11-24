//! `known_hosts` compatible con OpenSSH: lee entradas en claro y hasheadas
//! (`|1|salt|hash`), `[host]:port` para puertos distintos de 22, y marcadores
//! `@revoked`. Escribe en claro, de forma atómica y con bloqueo de fichero
//! (`<known_hosts>.lock`) alrededor de leer-modificar-renombrar.

use std::path::{Path, PathBuf};

use base64::Engine as _;
use base64::engine::general_purpose::{STANDARD, STANDARD_NO_PAD};
use hmac::{Hmac, KeyInit, Mac};
use sha1::Sha1;
use sha2::{Digest, Sha256};

use crate::Result;

/// Clave pública de host en formato OpenSSH (`algoritmo base64`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostKey {
    pub algorithm: String,
    pub base64: String,
}

impl HostKey {
    /// `SHA256:<base64 sin relleno>`, igual que `ssh-keygen -lf`.
    pub fn fingerprint_sha256(&self) -> String {
        // Blob ilegible: se huella el texto tal cual, sin pánico.
        let blob = STANDARD
            .decode(&self.base64)
            .unwrap_or_else(|_| self.base64.as_bytes().to_vec());
        format!("SHA256:{}", STANDARD_NO_PAD.encode(Sha256::digest(blob)))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostKeyStatus {
    Known,
    Unknown,
    /// Hay otra clave del mismo algoritmo para ese host.
    Changed {
        previous: HostKey,
    },
    Revoked,
}

enum Pattern {
    Plain { glob: String, negated: bool },
    Hashed { salt: Vec<u8>, hash: Vec<u8> },
}

struct Entry {
    patterns: Vec<Pattern>,
    revoked: bool,
    key: HostKey,
}

pub struct KnownHosts {
    path: PathBuf,
    entries: Vec<Entry>,
}

/// Nombre tal como OpenSSH lo busca: `host` en el 22, `[host]:port` en el resto.
fn host_token(host: &str, port: u16) -> String {
    if port == 22 {
        host.to_string()
    } else {
        format!("[{host}]:{port}")
    }
}

fn glob_match(pat: &[u8], s: &[u8]) -> bool {
    match pat.split_first() {
        None => s.is_empty(),
        Some((b'*', rest)) => (0..=s.len()).any(|i| glob_match(rest, &s[i..])),
        Some((b'?', rest)) => !s.is_empty() && glob_match(rest, &s[1..]),
        Some((c, rest)) => s.first() == Some(c) && glob_match(rest, &s[1..]),
    }
}

impl Pattern {
    fn parse(p: &str) -> Option<Pattern> {
        if let Some(rest) = p.strip_prefix("|1|") {
            let (salt, hash) = rest.split_once('|')?;
            return Some(Pattern::Hashed {
                salt: STANDARD.decode(salt).ok()?,
                hash: STANDARD.decode(hash).ok()?,
            });
        }
        let (negated, glob) = match p.strip_prefix('!') {
            Some(g) => (true, g),
            None => (false, p),
        };
        (!glob.is_empty()).then(|| Pattern::Plain {
            glob: glob.to_ascii_lowercase(),
            negated,
        })
    }

    fn matches(&self, token: &str) -> bool {
        match self {
            Pattern::Plain { glob, .. } => {
                glob_match(glob.as_bytes(), token.to_ascii_lowercase().as_bytes())
            }
            Pattern::Hashed { salt, hash } => match Hmac::<Sha1>::new_from_slice(salt) {
                Ok(mut mac) => {
                    // OpenSSH hashea el nombre en minúsculas.
                    mac.update(token.to_ascii_lowercase().as_bytes());
                    mac.verify_slice(hash).is_ok()
                }
                Err(_) => false,
            },
        }
    }
}

impl Entry {
    /// Línea mal formada o con marcador no soportado (`@cert-authority`) = `None`.
    fn parse(line: &str) -> Option<Entry> {
        let mut it = line.split_whitespace();
        let mut first = it.next()?;
        if first.starts_with('#') {
            return None;
        }
        let mut revoked = false;
        if first.starts_with('@') {
            if first != "@revoked" {
                return None;
            }
            revoked = true;
            first = it.next()?;
        }
        let algorithm = it.next()?;
        let b64 = it.next()?;
        let patterns: Vec<Pattern> = first.split(',').filter_map(Pattern::parse).collect();
        if patterns.is_empty() {
            return None;
        }
        Some(Entry {
            patterns,
            revoked,
            key: HostKey {
                algorithm: algorithm.into(),
                base64: b64.into(),
            },
        })
    }

    fn applies_to(&self, token: &str) -> bool {
        let mut hit = false;
        for p in &self.patterns {
            let m = p.matches(token);
            if matches!(p, Pattern::Plain { negated: true, .. }) {
                if m {
                    return false;
                }
            } else {
                hit |= m;
            }
        }
        hit
    }
}

impl KnownHosts {
    /// Fichero inexistente = vacío. Las líneas ilegibles se ignoran.
    pub fn load(path: &Path) -> Result<Self> {
        let text = read_lossy(path)?;
        let entries = text.lines().filter_map(Entry::parse).collect();
        Ok(Self {
            path: path.to_path_buf(),
            entries,
        })
    }

    /// Algoritmos de las claves no revocadas que aplican al host, sin duplicados
    /// y en orden de aparición (para ofrecer primero los ya conocidos).
    pub fn algorithms_for(&self, host: &str, port: u16) -> Vec<String> {
        if !valid_host(host) {
            return Vec::new();
        }
        let token = host_token(host, port);
        let mut out: Vec<String> = Vec::new();
        for e in self
            .entries
            .iter()
            .filter(|e| !e.revoked && e.applies_to(&token))
        {
            if !out.contains(&e.key.algorithm) {
                out.push(e.key.algorithm.clone());
            }
        }
        out
    }

    pub fn check(&self, host: &str, port: u16, key: &HostKey) -> HostKeyStatus {
        if !valid_host(host) {
            return HostKeyStatus::Unknown;
        }
        let token = host_token(host, port);
        let mut same_alg = None;
        let mut other_alg = None;
        let mut known = false;
        for e in self.entries.iter().filter(|e| e.applies_to(&token)) {
            let same = e.key == *key;
            if e.revoked {
                if same {
                    return HostKeyStatus::Revoked;
                }
            } else if same {
                known = true;
            } else if e.key.algorithm == key.algorithm {
                same_alg.get_or_insert_with(|| e.key.clone());
            } else {
                other_alg.get_or_insert_with(|| e.key.clone());
            }
        }
        // Una clave de otro algoritmo ya guardada también es un cambio: si no,
        // un intermediario ofrecería otro algoritmo para evitar el aviso fuerte.
        match (known, same_alg.or(other_alg)) {
            (true, _) => HostKeyStatus::Known,
            (false, Some(previous)) => HostKeyStatus::Changed { previous },
            (false, None) => HostKeyStatus::Unknown,
        }
    }

    pub fn add(&mut self, host: &str, port: u16, key: &HostKey) -> Result<()> {
        if !valid_host(host) {
            return Err(crate::Error::Parse {
                path: self.path.display().to_string(),
                msg: format!("nombre de host no válido para known_hosts: {host:?}"),
            });
        }
        let token = host_token(host, port);
        let path = self.path.clone();
        crate::fsutil::with_lock(&path, || {
            let mut text = read_lossy(&path)?;
            if !text.is_empty() && !text.ends_with('\n') {
                text.push('\n');
            }
            text.push_str(&format!("{token} {} {}\n", key.algorithm, key.base64));
            crate::fsutil::write_atomic(&path, text.as_bytes())
        })?;
        self.entries.push(Entry {
            patterns: vec![Pattern::Plain {
                glob: token.to_ascii_lowercase(),
                negated: false,
            }],
            revoked: false,
            key: key.clone(),
        });
        Ok(())
    }
}

/// Rechaza lo que rompería la línea o se interpretaría como patrón/marcador.
fn valid_host(host: &str) -> bool {
    !host.is_empty()
        && !host
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || ",*?!|#@".contains(c))
}

fn read_lossy(path: &Path) -> Result<String> {
    match std::fs::read(path) {
        Ok(b) => Ok(String::from_utf8_lossy(&b).into_owned()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(e) => Err(e.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const K1: &str = "AAAAC3NzaC1lZDI1NTE5AAAAIC2ODL4zdmXhYNYxexDmQYCJ8al+ghaQyLRM0NVdAel0";
    const K2: &str = "AAAAC3NzaC1lZDI1NTE5AAAAIOJKuS+jk02uvagqfliAjDxWNIIyN47rvkv7gzvjs6L9";
    // Generado con: printf example.com | openssl dgst -sha1 -hmac saltsaltsaltsaltsalt -binary | base64
    const HASHED: &str = "|1|c2FsdHNhbHRzYWx0c2FsdHNhbHQ=|FQ+T8llbOEZw6kMi1xhxPdATPO4=";

    fn key(b: &str) -> HostKey {
        HostKey {
            algorithm: "ssh-ed25519".into(),
            base64: b.into(),
        }
    }

    fn load(content: &str) -> (tempfile::TempDir, KnownHosts) {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("known_hosts");
        std::fs::write(&p, content).unwrap();
        let kh = KnownHosts::load(&p).unwrap();
        (d, kh)
    }

    #[test]
    fn fingerprint_matches_ssh_keygen() {
        assert_eq!(
            key(K1).fingerprint_sha256(),
            "SHA256:dufCjCWzgE/cCaJLuAOaFy7rOr9HOq/CgB50dC/FVcM"
        );
        assert_eq!(
            key(K2).fingerprint_sha256(),
            "SHA256:gjBKagij/fMhsxF7yQr5ueFhyoR76o/2QVIgayNV6gc"
        );
    }

    #[test]
    fn missing_file_is_empty() {
        let d = tempfile::tempdir().unwrap();
        let kh = KnownHosts::load(&d.path().join("nope")).unwrap();
        assert_eq!(kh.check("a", 22, &key(K1)), HostKeyStatus::Unknown);
    }

    #[test]
    fn plain_changed_and_unknown() {
        let (_d, kh) = load(&format!("host.example ssh-ed25519 {K1}\n"));
        assert_eq!(kh.check("host.example", 22, &key(K1)), HostKeyStatus::Known);
        assert_eq!(kh.check("HOST.example", 22, &key(K1)), HostKeyStatus::Known);
        assert_eq!(
            kh.check("host.example", 22, &key(K2)),
            HostKeyStatus::Changed { previous: key(K1) }
        );
        assert_eq!(kh.check("other", 22, &key(K1)), HostKeyStatus::Unknown);
        // Otro algoritmo con clave guardada: también es un cambio (anti-downgrade).
        let rsa = HostKey {
            algorithm: "ssh-rsa".into(),
            base64: "AAAA".into(),
        };
        assert_eq!(
            kh.check("host.example", 22, &rsa),
            HostKeyStatus::Changed { previous: key(K1) }
        );
    }

    #[test]
    fn algorithms_for_dedups_in_order_and_skips_revoked() {
        let (_d, kh) = load(&format!(
            "h ssh-ed25519 {K1}\nh ecdsa-sha2-nistp256 AAAA\nh ssh-ed25519 {K2}\n@revoked h ssh-rsa BBBB\nother ssh-rsa CCCC\n"
        ));
        assert_eq!(
            kh.algorithms_for("h", 22),
            vec!["ssh-ed25519", "ecdsa-sha2-nistp256"]
        );
        assert!(kh.algorithms_for("zzz", 22).is_empty());
        assert!(kh.algorithms_for("a,*", 22).is_empty());
    }

    #[test]
    fn hashed_entries_are_case_insensitive() {
        let (_d, kh) = load(&format!("{HASHED} ssh-ed25519 {K1}\n"));
        assert_eq!(kh.check("EXAMPLE.com", 22, &key(K1)), HostKeyStatus::Known);
    }

    #[test]
    fn rejects_injection_in_host() {
        let (d, mut kh) = load(&format!("* ssh-ed25519 {K1}\n"));
        for bad in [
            "evil\n* ssh-ed25519 CCCC\n#",
            "a,*",
            "",
            "a b",
            "a?",
            "!a",
            "|1|x",
            "#a",
            "@a",
        ] {
            assert!(matches!(
                kh.add(bad, 22, &key(K2)),
                Err(crate::Error::Parse { .. })
            ));
            // Aunque haya un comodín que cubriría todo, un host inválido es Unknown.
            assert_eq!(kh.check(bad, 22, &key(K1)), HostKeyStatus::Unknown);
        }
        let text = std::fs::read_to_string(d.path().join("known_hosts")).unwrap();
        assert_eq!(text, format!("* ssh-ed25519 {K1}\n"));
    }

    #[test]
    fn concurrent_adds_do_not_lose_entries() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("known_hosts");
        let handles: Vec<_> = (0..8)
            .map(|i| {
                let p = p.clone();
                std::thread::spawn(move || {
                    let mut kh = KnownHosts::load(&p).unwrap();
                    kh.add(&format!("h{i}"), 22, &key(K1)).unwrap();
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
        let kh = KnownHosts::load(&p).unwrap();
        for i in 0..8 {
            assert_eq!(
                kh.check(&format!("h{i}"), 22, &key(K1)),
                HostKeyStatus::Known
            );
        }
    }

    #[test]
    fn hashed_entry_known_vector() {
        let (_d, kh) = load(&format!("{HASHED} ssh-ed25519 {K1}\n"));
        assert_eq!(kh.check("example.com", 22, &key(K1)), HostKeyStatus::Known);
        assert_eq!(
            kh.check("example.org", 22, &key(K1)),
            HostKeyStatus::Unknown
        );
        assert_eq!(
            kh.check("example.com", 22, &key(K2)),
            HostKeyStatus::Changed { previous: key(K1) }
        );
    }

    #[test]
    fn nonstandard_port_and_lists() {
        let (_d, kh) = load(&format!("[h.example]:2222,10.0.0.1 ssh-ed25519 {K1}\n"));
        assert_eq!(kh.check("h.example", 2222, &key(K1)), HostKeyStatus::Known);
        assert_eq!(kh.check("h.example", 22, &key(K1)), HostKeyStatus::Unknown);
        assert_eq!(
            kh.check("h.example", 2223, &key(K1)),
            HostKeyStatus::Unknown
        );
        assert_eq!(kh.check("10.0.0.1", 22, &key(K1)), HostKeyStatus::Known);
    }

    #[test]
    fn revoked_comments_malformed_and_ca() {
        let content = format!(
            "# comentario\n\nbasura\nsolo dos\n@cert-authority *.x ssh-ed25519 {K1}\n@revoked bad ssh-ed25519 {K2}\nok ssh-ed25519 {K1}\n|1|%%%|%%% ssh-ed25519 {K1}\n"
        );
        let (_d, kh) = load(&content);
        assert_eq!(kh.check("bad", 22, &key(K2)), HostKeyStatus::Revoked);
        assert_eq!(kh.check("ok", 22, &key(K1)), HostKeyStatus::Known);
        // La CA se ignora.
        assert_eq!(kh.check("a.x", 22, &key(K1)), HostKeyStatus::Unknown);
    }

    #[test]
    fn wildcards_and_negation() {
        let (_d, kh) = load(&format!("*.corp.test,!secret.corp.test ssh-ed25519 {K1}\n"));
        assert_eq!(kh.check("a.corp.test", 22, &key(K1)), HostKeyStatus::Known);
        assert_eq!(
            kh.check("secret.corp.test", 22, &key(K1)),
            HostKeyStatus::Unknown
        );
    }

    #[test]
    fn add_writes_plain_atomically_and_roundtrips() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("sub").join("known_hosts");
        let mut kh = KnownHosts::load(&p).unwrap();
        kh.add("h", 22, &key(K1)).unwrap();
        kh.add("h2", 2222, &key(K2)).unwrap();
        assert_eq!(kh.check("h", 22, &key(K1)), HostKeyStatus::Known);
        let text = std::fs::read_to_string(&p).unwrap();
        assert_eq!(
            text,
            format!("h ssh-ed25519 {K1}\n[h2]:2222 ssh-ed25519 {K2}\n")
        );
        let kh2 = KnownHosts::load(&p).unwrap();
        assert_eq!(kh2.check("h2", 2222, &key(K2)), HostKeyStatus::Known);
        // Sin ficheros temporales sobrantes (solo datos + .lock).
        assert_eq!(std::fs::read_dir(p.parent().unwrap()).unwrap().count(), 2);
    }

    #[test]
    fn add_appends_newline_if_missing() {
        let (d, mut kh) = load(&format!("a ssh-ed25519 {K1}"));
        kh.add("b", 22, &key(K2)).unwrap();
        let text = std::fs::read_to_string(d.path().join("known_hosts")).unwrap();
        assert_eq!(text, format!("a ssh-ed25519 {K1}\nb ssh-ed25519 {K2}\n"));
    }
}
