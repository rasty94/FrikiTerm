import type { SessionConfig } from "./types";

/** Cada palabra de la consulta debe aparecer (sin distinguir mayúsculas) en nombre, host, usuario, carpeta o protocolo. */
export function filterHosts(hosts: SessionConfig[], query: string): SessionConfig[] {
  const terms = query.trim().toLowerCase().split(/\s+/).filter(Boolean);
  if (terms.length === 0) return hosts.slice();
  return hosts.filter((h) => {
    const hay = [h.name, h.host, h.user ?? "", h.folder, h.protocol].join(" ").toLowerCase();
    return terms.every((t) => hay.includes(t));
  });
}
