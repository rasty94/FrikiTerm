import { expect, test } from "vitest";
import { broadcastTargets, gridLayout, leavesOf, parseWorkspace, pushHistory, reorder, serializeNode, serializeWorkspace } from "./layout";
import type { Leaf, PaneNode, Tab } from "./state.svelte";

const leaf = (id: number): PaneNode => ({
  kind: "leaf",
  id,
  title: `t${id}`,
  target: { kind: "local" },
  status: { state: "connecting" },
  log: null,
});

test("una sola hoja ocupa toda la rejilla", () => {
  const l = gridLayout(leaf(1));
  expect(l.columns).toBe("minmax(0,1000000fr)");
  expect(l.slots[1]).toEqual({ column: "1 / 2", row: "1 / 2" });
});

test("división en fila y columna anidada: cada hoja tiene su celda", () => {
  const root: PaneNode = {
    kind: "split",
    id: 10,
    dir: "row",
    children: [leaf(1), { kind: "split", id: 11, dir: "col", children: [leaf(2), leaf(3)] }],
  };
  const l = gridLayout(root);
  expect(l.columns.split(" ")).toHaveLength(2);
  expect(l.rows.split(" ")).toHaveLength(2);
  expect(l.slots[1]).toEqual({ column: "1 / 2", row: "1 / 3" });
  expect(l.slots[2]).toEqual({ column: "2 / 3", row: "1 / 2" });
  expect(l.slots[3]).toEqual({ column: "2 / 3", row: "2 / 3" });
});

const open = (id: number, over: Partial<Leaf> = {}): Leaf =>
  ({ ...(leaf(id) as Leaf), status: { state: "open", negotiated: null, warnings: [] }, ...over });
const quick = (id: number): Leaf => ({
  ...(leaf(id) as Leaf),
  target: { kind: "quick", spec: { host: "h", port: 2200, user: "u", password: "nunca" } as never },
});
const split = (...children: PaneNode[]): PaneNode => ({ kind: "split", id: 99, dir: "row", children });
const tab = (id: number, root: PaneNode, broadcast = false): Tab => ({ id, title: `T${id}`, root, activeLeaf: 1, broadcast });

test("reorder mueve sin mutar y tolera índices inválidos", () => {
  const a = [1, 2, 3, 4];
  expect(reorder(a, 0, 2)).toEqual([2, 3, 1, 4]);
  expect(reorder(a, 3, 0)).toEqual([4, 1, 2, 3]);
  expect(reorder(a, 1, 1)).toEqual(a);
  expect(reorder(a, -1, 2)).toEqual(a);
  expect(reorder(a, 0, 9)).toEqual(a);
  expect(a).toEqual([1, 2, 3, 4]);
});

test("difusión: solo paneles abiertos y distintos del origen; apagada no envía a nadie", () => {
  const ls = [open(1), open(2), leaf(3) as Leaf, open(4, { status: { state: "closed", text: "x" } }), open(5, { status: { state: "failed", text: "x" } }), open(6)];
  expect(broadcastTargets(ls, 1, true)).toEqual([2, 6]);
  expect(broadcastTargets(ls, 2, true)).toEqual([1, 6]);
  expect(broadcastTargets(ls, 1, false)).toEqual([]);
  expect(broadcastTargets([open(1)], 1, true)).toEqual([]);
});

test("historial de conexiones rápidas: 10 máximo, sin duplicados, reciente primero", () => {
  let h: string[] = [];
  for (let i = 0; i < 12; i++) h = pushHistory(h, `u@h${i}`);
  expect(h).toHaveLength(10);
  expect(h[0]).toBe("u@h11");
  h = pushHistory(h, "u@h5");
  expect(h[0]).toBe("u@h5");
  expect(h.filter((x) => x === "u@h5")).toHaveLength(1);
  expect(pushHistory(h, "  ")).toEqual(h);
});

test("serializar: guardados y local sí; rápidas solo con opt-in y sin secretos", () => {
  const saved = open(1, { target: { kind: "saved", name: "web-01" } });
  const root = split(saved, quick(2));
  expect(serializeNode(root, false)).toEqual({ k: "leaf", title: "t1", target: { kind: "saved", name: "web-01" } });
  const withQuick = serializeNode(root, true);
  expect(JSON.stringify(withQuick)).not.toContain("nunca");
  expect(JSON.stringify(withQuick)).not.toContain("password");
  expect(withQuick).toMatchObject({ k: "split", children: [{ k: "leaf" }, { k: "leaf", target: { kind: "quick", user: "u", host: "h", port: 2200 } }] });
  expect(serializeNode(quick(3), false)).toBeNull();
  expect(serializeWorkspace([tab(1, quick(3))], 1, false)).toBeNull();
});

test("restauración: ida y vuelta, pestaña activa y difusión", () => {
  const tabs = [tab(1, open(1, { target: { kind: "saved", name: "a" } })), tab(2, split(leaf(2), leaf(3)), true), tab(3, quick(4))];
  const ws = serializeWorkspace(tabs, 2, false)!;
  expect(ws.tabs).toHaveLength(2); // la rápida se omite
  expect(ws.active).toBe(1);
  expect(ws.tabs[1].broadcast).toBe(true);
  expect(parseWorkspace(JSON.parse(JSON.stringify(ws)))).toEqual(ws);
});

test("restauración: valida lo leído de localStorage", () => {
  const good = { v: 1, active: 0, tabs: [{ title: "a", broadcast: false, root: { k: "leaf", title: "a", target: { kind: "local" } } }] };
  expect(parseWorkspace(good)).toEqual(good);
  expect(parseWorkspace({ ...good, active: 7 })?.active).toBe(0);
  for (const bad of [
    null, "x", {}, { v: 2, tabs: good.tabs }, { v: 1, tabs: [] },
    { ...good, tabs: [{ ...good.tabs[0], title: "" }] },
    { ...good, tabs: [{ ...good.tabs[0], title: "a\x1b[31m" }] },
    { ...good, tabs: [{ ...good.tabs[0], root: { k: "leaf", title: "a", target: { kind: "serial" } } }] },
    { ...good, tabs: [{ ...good.tabs[0], root: { k: "leaf", title: "a", target: { kind: "quick", user: null, host: "a b", port: 22 } } }] },
    { ...good, tabs: [{ ...good.tabs[0], root: { k: "leaf", title: "a", target: { kind: "quick", user: null, host: "h", port: 70000 } } }] },
    { ...good, tabs: [{ ...good.tabs[0], root: { k: "split", dir: "diag", children: [] } }] },
    { ...good, tabs: Array(51).fill(good.tabs[0]) },
  ])
    expect(parseWorkspace(bad)).toBeNull();
  // Más de 2 niveles de división: rechazado.
  const l = { k: "leaf", title: "a", target: { kind: "local" } };
  const s = (c: unknown[]) => ({ k: "split", dir: "row", children: c });
  expect(parseWorkspace({ ...good, tabs: [{ ...good.tabs[0], root: s([l, s([l, s([l, l])])]) }] })).toBeNull();
  expect(parseWorkspace({ ...good, tabs: [{ ...good.tabs[0], root: s([l, s([l, l])]) }] })).not.toBeNull();
});

test("duplicar: el árbol serializado conserva diseño y destinos de las hojas", () => {
  const root = split(open(1, { target: { kind: "saved", name: "a" } }), quick(2));
  const copy = serializeNode(root, true)!;
  expect(copy).toMatchObject({ k: "split", dir: "row" });
  expect(leavesOf(root)).toHaveLength(2);
});
