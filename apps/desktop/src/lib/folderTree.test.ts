import { expect, test } from "vitest";
import { buildFolderTree, hostsInFolder, normFolder } from "./folderTree";
import { defaultSession } from "./session";
import type { SessionConfig } from "./types";

const mk = (host: string, folder: string): SessionConfig => defaultSession({ host, folder });

test("normFolder limpia barras y espacios", () => {
  expect(normFolder(" /a//b/ ")).toBe("a/b");
  expect(normFolder("")).toBe("");
});

test("buildFolderTree anida carpetas y cuenta hosts por subárbol", () => {
  const tree = buildFolderTree([mk("h1", "prod/web"), mk("h2", "prod"), mk("h3", "lab"), mk("h4", "")]);
  expect(tree.count).toBe(4);
  expect(tree.children.map((c) => c.name)).toEqual(["lab", "prod"]);
  const prod = tree.children[1];
  expect(prod.path).toBe("prod");
  expect(prod.count).toBe(2);
  expect(prod.children[0].path).toBe("prod/web");
  expect(prod.children[0].count).toBe(1);
});

test("hostsInFolder incluye subcarpetas; null devuelve todos", () => {
  const hosts = [mk("h1", "prod/web"), mk("h2", "prod"), mk("h3", "lab")];
  expect(hostsInFolder(hosts, null)).toHaveLength(3);
  expect(hostsInFolder(hosts, "prod").map((h) => h.host)).toEqual(["h1", "h2"]);
  expect(hostsInFolder(hosts, "pro")).toEqual([]);
});
