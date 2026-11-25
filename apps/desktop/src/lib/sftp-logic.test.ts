import { describe, expect, test } from "vitest";
import type { EntryDto } from "./types";
import { breadcrumb, formatSize, joinPath, parentPath, relBreadcrumb, relParent, rootLabel, sortEntries } from "./sftp-logic";

const e = (name: string, kind: EntryDto["kind"], size = 0, mtime: number | null = null): EntryDto => ({
  name,
  path: `/x/${name}`,
  kind,
  size,
  mtime,
  mode: null,
});

describe("sortEntries", () => {
  const list = [e("b.txt", "file", 10, 5), e("a", "dir", 0, 1), e("c.txt", "file", 1, 9), e("file10", "file", 2, 2), e("file2", "file", 3, 3)];

  test("carpetas primero, nombre natural asc", () => {
    const names = sortEntries(list, { key: "name", dir: "asc" }).map((x) => x.name);
    expect(names).toEqual(["a", "b.txt", "c.txt", "file2", "file10"]);
  });

  test("por tamaño desc dentro de cada grupo", () => {
    const names = sortEntries(list, { key: "size", dir: "desc" }).map((x) => x.name);
    expect(names).toEqual(["a", "b.txt", "file2", "file10", "c.txt"]);
  });

  test("no muta la entrada", () => {
    const copy = [...list];
    sortEntries(list, { key: "mtime", dir: "asc" });
    expect(list).toEqual(copy);
  });
});

describe("rutas", () => {
  test("joinPath evita barras dobles", () => {
    expect(joinPath("/", "etc")).toBe("/etc");
    expect(joinPath("/home/u", "f")).toBe("/home/u/f");
    expect(joinPath("/home/u/", "f")).toBe("/home/u/f");
  });

  test("parentPath sube hasta la raíz", () => {
    expect(parentPath("/home/u/docs")).toBe("/home/u");
    expect(parentPath("/home")).toBe("/");
    expect(parentPath("/")).toBe("/");
  });

  test("breadcrumb", () => {
    expect(breadcrumb("/home/u")).toEqual([
      { label: "/", path: "/" },
      { label: "home", path: "/home" },
      { label: "u", path: "/home/u" },
    ]);
    expect(breadcrumb("/")).toEqual([{ label: "/", path: "/" }]);
  });
});

describe("rutas locales relativas a la raíz", () => {
  test("joinPath desde la raíz relativa (\"\") no añade barra inicial", () => {
    expect(joinPath("", "docs")).toBe("docs");
    expect(joinPath("docs", "a.txt")).toBe("docs/a.txt");
  });

  test("relParent: la raíz es su propio padre; sube un nivel en anidadas", () => {
    expect(relParent("")).toBe("");
    expect(relParent("docs")).toBe("");
    expect(relParent("docs/2024/x")).toBe("docs/2024");
  });

  test("rootLabel toma el último segmento de la raíz absoluta", () => {
    expect(rootLabel("/Users/x")).toBe("x");
    expect(rootLabel("/Users/x/")).toBe("x");
    expect(rootLabel("/")).toBe("/");
  });

  test("relBreadcrumb: raíz con su nombre, luego segmentos relativos", () => {
    expect(relBreadcrumb("x", "")).toEqual([{ label: "x", path: "" }]);
    expect(relBreadcrumb("x", "docs/2024")).toEqual([
      { label: "x", path: "" },
      { label: "docs", path: "docs" },
      { label: "2024", path: "docs/2024" },
    ]);
  });
});

describe("formatSize", () => {
  test("unidades base 1024", () => {
    expect(formatSize(0)).toBe("0 B");
    expect(formatSize(1536)).toBe("1.5 KB");
    expect(formatSize(1024 * 1024 * 3)).toBe("3.0 MB");
    expect(formatSize(-1)).toBe("—");
  });
});
