# Cambios de contrato propuestos por friki-tui

- Resuelto: `friki_ssh::Negotiated::weak()` es público y la TUI lo usa (ya no hay copia).
- `friki-tui` añade API pública para poder probar la sesión sin terminal:
  `run_with`, `connect_ui`, `shell_session`, `Paths` y `Connected` (`run` no cambia).
