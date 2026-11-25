import type { Leaf, PaneNode } from "./state.svelte";

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
