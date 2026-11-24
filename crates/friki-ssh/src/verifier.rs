use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;

use friki_core::known_hosts::{HostKey, HostKeyStatus, KnownHosts};

use crate::{HostKeyDecision, HostKeyVerifier};

/// Verificador por defecto basado en `friki_core::known_hosts`.
pub struct KnownHostsVerifier<F> {
    pub path: PathBuf,
    /// Se llama con clave desconocida o cambiada; con `Changed` solo
    /// debe poder devolver `Reject` o `AcceptOnce` tras aviso explícito.
    pub ask: F,
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
        if decision == HostKeyDecision::AcceptAndSave && known.add(host, port, key).is_err() {
            // El usuario aceptó pero no se pudo guardar: se acepta solo esta vez.
            return HostKeyDecision::AcceptOnce;
        }
        decision
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
        let d = decide(status, |_| async {
            calls.fetch_add(1, Ordering::SeqCst);
            answer
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

    // Comprueba que la forma de F compila con un closure real.
    #[allow(dead_code)]
    fn compila() -> impl HostKeyVerifier {
        KnownHostsVerifier {
            path: PathBuf::new(),
            ask: |_h: &str,
                  _p: u16,
                  _k: &HostKey,
                  _s: HostKeyStatus|
             -> Pin<Box<dyn Future<Output = HostKeyDecision> + Send>> {
                Box::pin(async { HostKeyDecision::Reject })
            },
        }
    }
}
