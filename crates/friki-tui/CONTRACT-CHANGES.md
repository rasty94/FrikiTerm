# Cambios de contrato propuestos por friki-tui

- `friki_ssh::Negotiated::weak()` es privado; la TUI duplica su heurística (`is_legacy`)
  para el indicador LEGACY permanente. Conviene exponer `Negotiated::is_legacy()`
  (o `weak()` público) y borrar la copia.
- `friki-tui` añade API pública para poder probar la sesión sin terminal:
  `run_with`, `connect_ui`, `shell_session`, `Paths` y `Connected` (`run` no cambia).
