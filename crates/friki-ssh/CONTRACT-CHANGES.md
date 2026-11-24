# Cambios de contrato propuestos en friki-ssh (no autorizados de antemano)

- `KnownHostsVerifier<F>` gana un campo privado (avisos): ya no se construye con el literal
  `KnownHostsVerifier { path, ask }`, sino con `KnownHostsVerifier::new(path, ask)`.
- `HostKeyVerifier` gana dos métodos con implementación por defecto (`known_algorithms`, `warnings`);
  los verificadores existentes siguen compilando.
- `ShellWriter::write`/`resize` ya no esperan acuse del canal: encolan (cola de 256) y devuelven
  `Error::Other("cola de escritura llena")` si se llena, `Error::Network` si el canal murió. Un fallo
  de envío posterior al encolado se ve como `Network` en la siguiente llamada o como `Lost` en el lector.
- `ShellReader` acumula la salida sin límite de cola de eventos, pero si se acumulan más de 32 MiB
  sin leer emite `ShellEvent::Lost` y deja de leer ese canal (única forma de no bloquear la conexión,
  porque russh envía WINDOW_ADJUST al recibir, no al consumir).
- `zeroize::Zeroizing` se reexporta como `friki_ssh::Zeroizing`.
- `Connection::exec_streaming` recibe `stdin` sin `Option` (usa `tokio::io::empty()` si no hay entrada).
- La dependencia directa `ssh-key` desaparece: la generación de claves usa `russh::keys::ssh_key`.
- `known_algorithms` sondea con `KnownHosts::check` hasta que friki-core exponga `algorithms_for`
  (hay un TODO en `verifier.rs`); con el cambio de core (`Changed` para cualquier algoritmo distinto)
  el sondeo devolverá todos y la reordenación quedará neutra hasta migrar.
