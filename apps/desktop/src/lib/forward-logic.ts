// Validación de redirecciones (-L / -R / -D) antes de llamar a fwd_start. Rust valida otra vez.
import type { Forward } from "./types";

export type FwdKind = "local" | "remote" | "dynamic";

export interface FwdDraft {
  kind: FwdKind;
  bind: string;
  port: string;
  host: string;
  hostPort: string;
}

export type FwdResult = { ok: true; spec: Forward; warning: boolean } | { ok: false; error: string };

export function isLoopbackBind(bind: string): boolean {
  const b = bind.trim().toLowerCase();
  return b === "localhost" || b === "::1" || b === "[::1]" || b.startsWith("127.");
}

function parsePort(raw: string): number | null {
  if (!/^\d+$/.test(raw.trim())) return null;
  const n = Number(raw.trim());
  return n >= 1 && n <= 65535 ? n : null;
}

// Hostname o IPv4 o IPv6 entre corchetes/sin ellos. Sin espacios ni caracteres de comando.
const BIND_RE = /^(\[[0-9a-fA-F:.]+\]|[0-9a-fA-F:]+:[0-9a-fA-F:.]*|[A-Za-z0-9.-]+)$/;
const HOST_RE = /^[A-Za-z0-9._:\-\[\]]+$/;

export function validateForward(d: FwdDraft): FwdResult {
  const bind = d.bind.trim() || "127.0.0.1";
  if (!BIND_RE.test(bind)) return { ok: false, error: "Dirección de escucha no válida" };
  const port = parsePort(d.port);
  if (port === null) return { ok: false, error: "Puerto local debe estar entre 1 y 65535" };
  const warning = !isLoopbackBind(bind);

  if (d.kind === "dynamic") {
    return { ok: true, spec: { kind: "dynamic", bind, port }, warning };
  }
  const host = d.host.trim();
  if (!host || !HOST_RE.test(host)) return { ok: false, error: "Host de destino no válido" };
  const hostPort = parsePort(d.hostPort);
  if (hostPort === null) return { ok: false, error: "Puerto de destino debe estar entre 1 y 65535" };
  return { ok: true, spec: { kind: d.kind, bind, port, host, host_port: hostPort }, warning };
}
