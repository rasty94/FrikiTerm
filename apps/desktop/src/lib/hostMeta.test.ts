import { describe, expect, it } from "vitest";
import { ago, initialsOf, navTarget, osInfo, recentHosts, sortHosts } from "./hostMeta";
import type { SessionConfig } from "./types";

const h = (name: string) => ({ name }) as SessionConfig;
const hosts = [h("c"), h("a"), h("b"), h("d")];
const meta = { c: { lastConnected: 300, connectCount: 1 }, b: { lastConnected: 100, connectCount: 1 }, a: { connectCount: 0 } };

describe("orden y recientes", () => {
  it("ordena por nombre", () => expect(sortHosts(hosts, meta, "name").map((x) => x.name)).toEqual(["a", "b", "c", "d"]));
  it("ordena por uso: recientes primero, nunca usados por nombre", () =>
    expect(sortHosts(hosts, meta, "recent").map((x) => x.name)).toEqual(["c", "b", "a", "d"]));
  it("no muta la entrada", () => {
    sortHosts(hosts, meta, "name");
    expect(hosts[0].name).toBe("c");
  });
  it("recientes: solo los usados, con tope", () => {
    expect(recentHosts(hosts, meta).map((x) => x.name)).toEqual(["c", "b"]);
    expect(recentHosts(hosts, meta, 1).map((x) => x.name)).toEqual(["c"]);
    expect(recentHosts(hosts, {})).toEqual([]);
  });
});

describe("osInfo", () => {
  it("distros con insignia", () => {
    expect(osInfo("debian")).toMatchObject({ glyph: "linux", label: "Debian", badge: { letter: "D" } });
    expect(osInfo("Ubuntu").badge?.letter).toBe("U");
  });
  it("familias y desconocidos", () => {
    expect(osInfo("macos").glyph).toBe("apple");
    expect(osInfo("windows").glyph).toBe("windows");
    expect(osInfo("openbsd")).toMatchObject({ glyph: "bsd", label: "OpenBSD" });
    expect(osInfo("linux")).toMatchObject({ glyph: "linux", label: "Linux" });
    expect(osInfo("nixos")).toMatchObject({ glyph: "linux", label: "nixos" });
    expect(osInfo(undefined).glyph).toBe("server");
  });
});

describe("textos", () => {
  it("iniciales", () => {
    expect(initialsOf("router-casa")).toBe("RC");
    expect(initialsOf("web-01")).toBe("WE");
  });
  it("hace cuánto", () => {
    expect(ago(undefined)).toBe("nunca");
    expect(ago(1000, 1030)).toBe("ahora");
    expect(ago(1000, 1000 + 300)).toBe("hace 5 min");
    expect(ago(1000, 1000 + 7200)).toBe("hace 2 h");
    expect(ago(1000, 1000 + 3 * 86400)).toBe("hace 3 d");
  });
});

describe("navegación con flechas", () => {
  const list = [0, 1, 2].map((i) => ({ left: 0, top: i * 50 }));
  it("lista", () => {
    expect(navTarget("ArrowDown", 0, list, false)).toBe(1);
    expect(navTarget("ArrowUp", 0, list, false)).toBe(0);
    expect(navTarget("ArrowDown", 2, list, false)).toBe(2);
    expect(navTarget("End", 0, list, false)).toBe(2);
  });
  // 2 columnas, 3 filas; la última fila solo tiene un elemento.
  const grid = [
    { left: 0, top: 0 }, { left: 100, top: 0 },
    { left: 0, top: 50 }, { left: 100, top: 50 },
    { left: 0, top: 100 },
  ];
  it("cuadrícula", () => {
    expect(navTarget("ArrowRight", 0, grid, true)).toBe(1);
    expect(navTarget("ArrowDown", 1, grid, true)).toBe(3);
    expect(navTarget("ArrowDown", 3, grid, true)).toBe(4); // fila corta: el más cercano
    expect(navTarget("ArrowUp", 4, grid, true)).toBe(2);
    expect(navTarget("ArrowUp", 0, grid, true)).toBe(0);
  });
  it("sin foco previo empieza en el primero", () => expect(navTarget("ArrowDown", -1, grid, true)).toBe(0));
});
