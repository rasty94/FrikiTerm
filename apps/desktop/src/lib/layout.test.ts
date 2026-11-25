import { expect, test } from "vitest";
import { gridLayout } from "./layout";
import type { PaneNode } from "./state.svelte";

const leaf = (id: number): PaneNode => ({
  kind: "leaf",
  id,
  title: `t${id}`,
  target: { kind: "local" },
  status: { state: "connecting" },
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
