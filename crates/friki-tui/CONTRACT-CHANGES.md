# Cambios de contrato propuestos por friki-tui

- `friki_ssh::KnownHostsVerifier<F>` no tiene `impl HostKeyVerifier` ni firma de `ask`
  en el contrato, así que friki-tui implementa su propio verificador sobre
  `friki_core::known_hosts::KnownHosts` (check/add). Si friki-ssh publica el impl,
  conviene fijar `F: Fn(&str, u16, &HostKey, &HostKeyStatus) -> impl Future<Output = HostKeyDecision>`
  y migrar `UiVerifier`.
- Convención asumida: con `AcceptAndSave` la TUI ya escribe en known_hosts y devuelve
  `AcceptOnce` para que la capa SSH no duplique la entrada. Para claves `Known` devuelve
  `AcceptOnce` (el contrato no tiene "ya conocida").
