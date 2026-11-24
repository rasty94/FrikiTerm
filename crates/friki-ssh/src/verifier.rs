use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::{Mutex, PoisonError};

use friki_core::known_hosts::{HostKey, HostKeyStatus, KnownHosts};

use crate::{HostKeyDecision, HostKeyVerifier};

/// Verificador por defecto (y único) basado en `friki_core::known_hosts`.
///
/// Política: `Known` acepta sin preguntar; `Revoked` rechaza sin preguntar;
/// `Unknown` y `Changed` delegan en `ask`. Con `Changed` nunca se guarda (un
/// `AcceptAndSave` se degrada a `AcceptOnce`). Si el guardado falla se acepta
/// solo esta vez y queda un aviso en [`HostKeyVerifier::warnings`].
///
/// `ask(host, port, clave, estado)` devuelve un futuro (`Pin<Box<dyn Future + Send>>`).
///
/// Desde la TUI (diálogo; `dialog` es un canal/callback propio de la TUI):
///
/// ```ignore
/// let verifier = KnownHostsVerifier::new(known_hosts_path, move |host: &str, port, key: &HostKey, status| {
///     let (host, key, dialog) = (host.to_string(), key.clone(), dialog.clone());
///     Box::pin(async move {
///         // Muestra host:port, key.fingerprint_sha256() y, si `status` es `Changed`,
///         // un aviso de posible ataque. Devuelve la elección del usuario.
///         dialog.ask_host_key(&host, port, &key, status).await // Reject | AcceptOnce | AcceptAndSave
///     })
/// });
/// ```
///
/// Desde la CLI (no interactiva: rechaza siempre lo no conocido):
///
/// ```ignore
/// let verifier = KnownHostsVerifier::new(path, |_h: &str, _p: u16, _k: &HostKey, _s: HostKeyStatus| {
///     Box::pin(async { HostKeyDecision::Reject }) as Pin<Box<dyn Future<Output = HostKeyDecision> + Send>>
/// });
/// ```
pub struct KnownHostsVerifier<F> {
    pub path: PathBuf,
    /// Se llama con clave desconocida o cambiada; con `Changed` solo
    /// debe poder devolver `Reject` o `AcceptOnce` tras aviso explícito.
    pub ask: F,
    warnings: Mutex<Vec<String>>,
}

impl<F> KnownHostsVerifier<F> {
    pub fn new(path: PathBuf, ask: F) -> Self {
        Self {
            path,
            ask,
            warnings: Mutex::new(Vec::new()),
        }
    }

    fn warn(&self, w: String) {
        self.warnings
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(w);
    }
}

/// Política pura: `Known` y `Revoked` no preguntan; con `Changed` nunca se guarda.
pub(crate) async fn decide<Fut>(
    status: HostKeyStatus,
    ask: impl FnOnce(HostKeyStatus) -> Fut,
) -> HostKeyDecision
where
    Fut: Future<Output = HostKeyDecision>,
{
    match status {
        HostKeyStatus::Known => HostKeyDecision::AcceptOnce,
        HostKeyStatus::Revoked => HostKeyDecision::Reject,
        changed @ HostKeyStatus::Changed { .. } => match ask(changed).await {
            HostKeyDecision::AcceptAndSave => HostKeyDecision::AcceptOnce,
            d => d,
        },
        unknown => ask(unknown).await,
    }
}

#[async_trait::async_trait]
impl<F> HostKeyVerifier for KnownHostsVerifier<F>
where
    F: Fn(
            &str,
            u16,
            &HostKey,
            HostKeyStatus,
        ) -> Pin<Box<dyn Future<Output = HostKeyDecision> + Send>>
        + Send
        + Sync,
{
    async fn verify(&self, host: &str, port: u16, key: &HostKey) -> HostKeyDecision {
        // known_hosts ilegible: se cierra en falso en lugar de aceptar a ciegas.
        let Ok(mut known) = KnownHosts::load(&self.path) else {
            return HostKeyDecision::Reject;
        };
        let status = known.check(host, port, key);
        let decision = decide(status, |s| (self.ask)(host, port, key, s)).await;
        if decision == HostKeyDecision::AcceptAndSave
            && let Err(e) = known.add(host, port, key)
        {
            // El usuario aceptó pero no se pudo guardar: se acepta solo esta vez, con aviso.
            self.warn(format!(
                "no se pudo guardar la clave de host de {host}:{port} en {}: {e}",
                self.path.display()
            ));
            return HostKeyDecision::AcceptOnce;
        }
        decision
    }

    fn known_algorithms(&self, host: &str, port: u16) -> Vec<String> {
        KnownHosts::load(&self.path)
            .map(|known| known.algorithms_for(host, port))
            .unwrap_or_default()
    }

    fn warnings(&self) -> Vec<String> {
        self.warnings
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    fn key(s: &str) -> HostKey {
        HostKey {
            algorithm: "ssh-ed25519".into(),
            base64: s.into(),
        }
    }

    async fn run(status: HostKeyStatus, answer: HostKeyDecision) -> (HostKeyDecision, usize) {
        let calls = AtomicUsize::new(0);
        let d = decide(status, |_| {
            calls.fetch_add(1, Ordering::SeqCst);
            async move { answer }
        })
        .await;
        (d, calls.load(Ordering::SeqCst))
    }

    #[tokio::test]
    async fn known_acepta_sin_preguntar() {
        assert_eq!(
            run(HostKeyStatus::Known, HostKeyDecision::Reject).await,
            (HostKeyDecision::AcceptOnce, 0)
        );
    }

    #[tokio::test]
    async fn revoked_rechaza_sin_preguntar() {
        assert_eq!(
            run(HostKeyStatus::Revoked, HostKeyDecision::AcceptAndSave).await,
            (HostKeyDecision::Reject, 0)
        );
    }

    #[tokio::test]
    async fn unknown_delega() {
        let (d, n) = run(HostKeyStatus::Unknown, HostKeyDecision::AcceptAndSave).await;
        assert_eq!((d, n), (HostKeyDecision::AcceptAndSave, 1));
    }

    #[tokio::test]
    async fn changed_nunca_guarda() {
        let changed = || HostKeyStatus::Changed {
            previous: key("AAAA"),
        };
        assert_eq!(
            run(changed(), HostKeyDecision::AcceptAndSave).await,
            (HostKeyDecision::AcceptOnce, 1)
        );
        assert_eq!(
            run(changed(), HostKeyDecision::AcceptOnce).await,
            (HostKeyDecision::AcceptOnce, 1)
        );
        assert_eq!(
            run(changed(), HostKeyDecision::Reject).await,
            (HostKeyDecision::Reject, 1)
        );
    }

    type Ask = Pin<Box<dyn Future<Output = HostKeyDecision> + Send>>;
    type AskFn = fn(&str, u16, &HostKey, HostKeyStatus) -> Ask;

    fn accepting(path: PathBuf, answer: HostKeyDecision) -> KnownHostsVerifier<AskFn> {
        let ask: AskFn = if answer == HostKeyDecision::Reject {
            |_, _, _, _| Box::pin(async { HostKeyDecision::Reject })
        } else {
            |_, _, _, _| Box::pin(async { HostKeyDecision::AcceptAndSave })
        };
        KnownHostsVerifier::new(path, ask)
    }

    #[tokio::test]
    async fn guarda_conoce_y_ordena_algoritmos() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("known_hosts");
        let v = accepting(path.clone(), HostKeyDecision::AcceptAndSave);
        assert!(v.known_algorithms("h", 22).is_empty());
        let k = key("AAAAC3NzaC1lZDI1NTE5AAAAIKQ1Zk4P1o3K5mC2m7p0uZ2QY7u2m4yQ9J0yq0m0m0m0");
        assert_eq!(v.verify("h", 22, &k).await, HostKeyDecision::AcceptAndSave);
        assert_eq!(v.known_algorithms("h", 22), ["ssh-ed25519"]);
        assert!(v.known_algorithms("otro", 22).is_empty());
        assert!(v.warnings().is_empty());
        // Ya conocida: no vuelve a preguntar.
        let no = accepting(path, HostKeyDecision::Reject);
        assert_eq!(no.verify("h", 22, &k).await, HostKeyDecision::AcceptOnce);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn fallo_al_guardar_acepta_una_vez_con_aviso() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("known_hosts");
        std::fs::write(&path, "").expect("write");
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o500))
            .expect("chmod");
        let v = accepting(path, HostKeyDecision::AcceptAndSave);
        let d = v.verify("h", 22, &key("AAAA")).await;
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700))
            .expect("chmod");
        if d == HostKeyDecision::AcceptAndSave {
            return; // p. ej. root: el directorio es escribible igualmente
        }
        assert_eq!(d, HostKeyDecision::AcceptOnce);
        assert_eq!(v.warnings().len(), 1);
    }
}
