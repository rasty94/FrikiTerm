# securecrt

Análisis estático de SecureCRT/SecureFX 9.7.4 (VanDyke) con la skill `reverse-engineer-anything` (REA).

## Documentos

- [docs/00-arquitectura.md](docs/00-arquitectura.md): visión general
- [docs/01-protocolos-y-capacidades.md](docs/01-protocolos-y-capacidades.md)
- [docs/02-comparativa-plataformas.md](docs/02-comparativa-plataformas.md)
- [docs/03-auditoria-seguridad.md](docs/03-auditoria-seguridad.md)

## Reproducir

1. Colocar aquí el `.dmg`, el `.deb` y el `.exe` (ignorados por git) y extraerlos en `extracted/{macos,ubuntu,windows}`.
2. Registrar REA: `npx -y rea-agents@latest setup --client claude_code --yes`, y reiniciar Claude Code.
3. La skill vive en `.claude/skills/reverse-engineer-anything/`.

Solo análisis estático: no se ejecutó ningún binario del producto.
