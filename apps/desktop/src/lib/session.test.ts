import { expect, test } from "vitest";
import { parseQuickSpec } from "./session";

test("parseQuickSpec acepta usuario@host:puerto con partes opcionales", () => {
  expect(parseQuickSpec("ana@srv.lan:2222")).toEqual({ user: "ana", host: "srv.lan", port: 2222 });
  expect(parseQuickSpec("srv")).toEqual({ user: null, host: "srv", port: 22 });
});

test("parseQuickSpec rechaza entradas no válidas", () => {
  expect(parseQuickSpec("")).toBeNull();
  expect(parseQuickSpec("a@b:70000")).toBeNull();
  expect(parseQuickSpec("a@@b")).toBeNull();
});
