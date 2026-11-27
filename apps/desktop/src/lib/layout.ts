import type { Leaf, PaneNode, Tab } from "./state.svelte";

// Unidades enteras para que los bordes coincidan exactamente entre hermanos.
const SCALE = 1_000_000;

interface Box {
  x0: number;
  x1: number;
  y0: number;
  y1: number;
}

export function leavesOf(n: PaneNode): Leaf[] {
  return n.kind === "leaf" ? [n] : n.children.flatMap(leavesOf);
}

function place(n: PaneNode, b: Box, out: Map<number, Box>): void {
  if (n.kind === "leaf") {
    out.set(n.id, b);
    return;
  }
  const { dir, children } = n;
  const k = children.length;
  children.forEach((c, i) => {
    const pos = (lo: number, hi: number, j: number) => lo + Math.round(((hi - lo) * j) / k);
    place(
      c,
      dir === "row"
        ? { x0: pos(b.x0, b.x1, i), x1: pos(b.x0, b.x1, i + 1), y0: b.y0, y1: b.y1 }
        : { x0: b.x0, x1: b.x1, y0: pos(b.y0, b.y1, i), y1: pos(b.y0, b.y1, i + 1) },
      out,
    );
  });
}

export interface GridSlot {
  column: string;
  row: string;
}

/**
 * Árbol de divisiones -> pistas de un CSS grid plano. Cada hoja ocupa su
 * celda con grid-column/grid-row; así los terminales no se re-montan al dividir.
 */
export function gridLayout(root: PaneNode): { columns: string; rows: string; slots: Record<number, GridSlot> } {
  const boxes = new Map<number, Box>();
  place(root, { x0: 0, x1: SCALE, y0: 0, y1: SCALE }, boxes);
  const xs = [...new Set([...boxes.values()].flatMap((b) => [b.x0, b.x1]))].sort((a, b) => a - b);
  const ys = [...new Set([...boxes.values()].flatMap((b) => [b.y0, b.y1]))].sort((a, b) => a - b);
  const track = (edges: number[]) =>
    edges.slice(1).map((e, i) => `minmax(0,${e - edges[i]}fr)`).join(" ");
  const slots: Record<number, GridSlot> = {};
  for (const [id, b] of boxes) {
    slots[id] = {
      column: `${xs.indexOf(b.x0) + 1} / ${xs.indexOf(b.x1) + 1}`,
      row: `${ys.indexOf(b.y0) + 1} / ${ys.indexOf(b.y1) + 1}`,
    };
  }
  return { columns: track(xs), rows: track(ys), slots };
}

// ---- Pestañas: orden, difusión de entrada y restauración del espacio de trabajo ----

/** Copia de `arr` con el elemento `from` movido a la posición `to` (índices fuera de rango: sin cambios). */
export function reorder<T>(arr: readonly T[], from: number, to: number): T[] {
  const out = [...arr];
  if (from === to || from < 0 || to < 0 || from >= out.length || to >= out.length) return out;
  out.splice(to, 0, ...out.splice(from, 1));
  return out;
}

/**
 * Paneles que reciben la entrada del panel `source` además de él mismo: con la difusión activa,
 * los demás de la pestaña con sesión abierta (nunca cerrados, fallidos ni aún conectando).
 */
export function broadcastTargets(leaves: readonly Leaf[], source: number, on: boolean): number[] {
  if (!on) return [];
  return leaves.filter((l) => l.id !== source && l.status.state === "open").map((l) => l.id);
}

/** Últimas `max` conexiones rápidas, sin duplicados y la más reciente primero. */
export function pushHistory(list: readonly string[], spec: string, max = 10): string[] {
  const s = spec.trim();
  return s ? [s, ...list.filter((x) => x !== s)].slice(0, max) : [...list];
}

/** Destinos persistibles: nunca contraseñas ni configuración de host (solo nombre guardado o usuario@host:puerto). */
export type SavedTarget =
  | { kind: "saved"; name: string }
  | { kind: "local" }
  | { kind: "quick"; user: string | null; host: string; port: number };
export type SavedNode =
  | { k: "leaf"; title: string; target: SavedTarget }
  | { k: "split"; dir: "row" | "col"; children: SavedNode[] };
export interface SavedTab {
  title: string;
  broadcast: boolean;
  root: SavedNode;
}
export interface SavedWorkspace {
  v: 1;
  active: number;
  tabs: SavedTab[];
}

const MAX_SAVED_TABS = 50;
const MAX_SAVED_DEPTH = 2;

/** Árbol -> forma persistible. Sin `includeQuick` las hojas rápidas se omiten; null si no queda ninguna. */
export function serializeNode(n: PaneNode, includeQuick: boolean): SavedNode | null {
  if (n.kind === "leaf") {
    const t = n.target;
    if (t.kind === "saved") return { k: "leaf", title: n.title, target: { kind: "saved", name: t.name } };
    if (t.kind === "local") return { k: "leaf", title: n.title, target: { kind: "local" } };
    return includeQuick
      ? { k: "leaf", title: n.title, target: { kind: "quick", user: t.spec.user, host: t.spec.host, port: t.spec.port } }
      : null;
  }
  const kids = n.children.map((c) => serializeNode(c, includeQuick)).filter((c): c is SavedNode => c !== null);
  if (kids.length === 0) return null;
  return kids.length === 1 ? kids[0] : { k: "split", dir: n.dir, children: kids };
}

export function serializeWorkspace(tabs: readonly Tab[], activeId: number, includeQuick: boolean): SavedWorkspace | null {
  const out: SavedTab[] = [];
  let active = 0;
  for (const t of tabs) {
    const root = serializeNode(t.root, includeQuick);
    if (!root) continue;
    if (t.id === activeId) active = out.length;
    out.push({ title: t.title, broadcast: t.broadcast, root });
  }
  return out.length ? { v: 1, active, tabs: out } : null;
}

const str = (v: unknown, max = 200): v is string =>
  typeof v === "string" && v.length > 0 && v.length <= max && !/\p{Cc}/u.test(v);

function parseTarget(r: any): SavedTarget | null {
  if (!r || typeof r !== "object") return null;
  if (r.kind === "local") return { kind: "local" };
  if (r.kind === "saved") return str(r.name) ? { kind: "saved", name: r.name } : null;
  if (r.kind === "quick") {
    const ok = str(r.host) && !/\s|@/.test(r.host) && Number.isInteger(r.port) && r.port >= 1 && r.port <= 65535;
    return ok && (r.user === null || str(r.user)) ? { kind: "quick", user: r.user, host: r.host, port: r.port } : null;
  }
  return null;
}

function parseNode(r: any, depth: number): SavedNode | null {
  if (!r || typeof r !== "object") return null;
  if (r.k === "leaf") {
    const target = parseTarget(r.target);
    return target && str(r.title) ? { k: "leaf", title: r.title, target } : null;
  }
  if (r.k !== "split" || depth >= MAX_SAVED_DEPTH || (r.dir !== "row" && r.dir !== "col")) return null;
  if (!Array.isArray(r.children) || r.children.length < 2 || r.children.length > 8) return null;
  const children = r.children.map((c: unknown) => parseNode(c, depth + 1));
  return children.every(Boolean) ? { k: "split", dir: r.dir, children } : null;
}

/** Valida lo leído de localStorage (puede estar corrupto o manipulado); null si no sirve. */
export function parseWorkspace(raw: unknown): SavedWorkspace | null {
  const r = raw as any;
  if (!r || r.v !== 1 || !Array.isArray(r.tabs) || r.tabs.length === 0 || r.tabs.length > MAX_SAVED_TABS) return null;
  const tabs: SavedTab[] = [];
  for (const t of r.tabs) {
    const root = parseNode(t?.root, 0);
    if (!root || !str(t.title)) return null;
    tabs.push({ title: t.title, broadcast: t.broadcast === true, root });
  }
  const active = Number.isInteger(r.active) && r.active >= 0 && r.active < tabs.length ? r.active : 0;
  return { v: 1, active, tabs };
}
