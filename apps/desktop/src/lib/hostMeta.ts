import type { HostMeta, SessionConfig } from "./types";
import type { IconName } from "./icons";

export type MetaMap = Record<string, HostMeta>;
export type SortMode = "name" | "recent";
export type ViewMode = "list" | "grid";

export const RECENT_COUNT = 5;

const byName = (a: SessionConfig, b: SessionConfig) => a.name.localeCompare(b.name);

/** "name": alfabético. "recent": última conexión primero; los nunca usados al final, por nombre. */
export function sortHosts(hosts: SessionConfig[], meta: MetaMap, mode: SortMode): SessionConfig[] {
  const last = (h: SessionConfig) => meta[h.name]?.lastConnected ?? 0;
  return hosts.slice().sort(mode === "recent" ? (a, b) => last(b) - last(a) || byName(a, b) : byName);
}

/** Los `n` hosts conectados más recientemente (los que nunca se usaron no cuentan). */
export function recentHosts(hosts: SessionConfig[], meta: MetaMap, n = RECENT_COUNT): SessionConfig[] {
  return sortHosts(hosts.filter((h) => meta[h.name]?.lastConnected), meta, "recent").slice(0, n);
}

export interface OsInfo {
  glyph: IconName;
  label: string;
  /** Distros: letra y tono OKLCH de la insignia de color. */
  badge?: { letter: string; hue: number };
}

const DISTROS: Record<string, [string, string, number]> = {
  debian: ["Debian", "D", 15],
  ubuntu: ["Ubuntu", "U", 50],
  alpine: ["Alpine", "A", 235],
  rhel: ["RHEL", "R", 25],
  centos: ["CentOS", "C", 300],
  fedora: ["Fedora", "F", 255],
  arch: ["Arch", "A", 215],
  opensuse: ["openSUSE", "S", 150],
  raspbian: ["Raspbian", "R", 5],
};

/** SO detectado (`HostMeta.os`) -> glifo y rótulo. Sin SO: icono de servidor genérico. */
export function osInfo(os?: string): OsInfo {
  const k = (os ?? "").toLowerCase();
  if (DISTROS[k]) {
    const [label, letter, hue] = DISTROS[k];
    return { glyph: "linux", label, badge: { letter, hue } };
  }
  if (k === "macos") return { glyph: "apple", label: "macOS" };
  if (k === "windows") return { glyph: "windows", label: "Windows" };
  if (k.endsWith("bsd")) return { glyph: "bsd", label: k.replace("bsd", "BSD").replace(/^./, (c) => c.toUpperCase()) };
  if (k) return { glyph: "linux", label: k === "linux" ? "Linux" : k };
  return { glyph: "server", label: "SO sin detectar" };
}

/** "router-casa" -> "RC"; "web-01" -> "WE" (un sufijo numérico no aporta). */
export function initialsOf(name: string): string {
  const [a = "", b = ""] = name.split(/[^\p{L}\p{N}]+/u).filter(Boolean);
  return (/^\p{L}/u.test(b) ? a[0] + b[0] : a.slice(0, 2)).toUpperCase() || "?";
}

/** "hace 5 min", "hace 2 d"; sin dato, "nunca". */
export function ago(secs: number | undefined, now = Date.now() / 1000): string {
  if (!secs) return "nunca";
  const d = Math.max(0, now - secs);
  if (d < 60) return "ahora";
  if (d < 3600) return `hace ${Math.floor(d / 60)} min`;
  if (d < 86400) return `hace ${Math.floor(d / 3600)} h`;
  return `hace ${Math.floor(d / 86400)} d`;
}

/** Siguiente elemento al pulsar una flecha. Lista: arriba/abajo. Cuadrícula: además izquierda/derecha,
 * y arriba/abajo al elemento de la fila contigua más cercano en horizontal. `rects` = posiciones. */
export function navTarget(
  key: string,
  i: number,
  rects: { left: number; top: number }[],
  grid: boolean,
): number {
  const n = rects.length;
  if (n === 0 || i < 0) return n ? 0 : -1;
  if (key === "Home") return 0;
  if (key === "End") return n - 1;
  if (!grid) return key === "ArrowDown" ? Math.min(n - 1, i + 1) : key === "ArrowUp" ? Math.max(0, i - 1) : i;
  if (key === "ArrowRight") return Math.min(n - 1, i + 1);
  if (key === "ArrowLeft") return Math.max(0, i - 1);
  const dir = key === "ArrowDown" ? 1 : key === "ArrowUp" ? -1 : 0;
  if (!dir) return i;
  const cur = rects[i];
  // Fila contigua: la `top` más próxima en esa dirección.
  const tops = [...new Set(rects.map((r) => r.top))].filter((t) => (t - cur.top) * dir > 0);
  if (!tops.length) return i;
  const row = tops.reduce((a, b) => (Math.abs(a - cur.top) < Math.abs(b - cur.top) ? a : b));
  let best = i;
  let dist = Infinity;
  rects.forEach((r, j) => {
    if (r.top === row && Math.abs(r.left - cur.left) < dist) {
      dist = Math.abs(r.left - cur.left);
      best = j;
    }
  });
  return best;
}
