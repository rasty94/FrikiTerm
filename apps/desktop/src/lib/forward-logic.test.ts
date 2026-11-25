import { describe, expect, test } from "vitest";
import { isLoopbackBind, validateForward, type FwdDraft } from "./forward-logic";

const base: FwdDraft = { kind: "local", bind: "", port: "8080", host: "db.internal", hostPort: "5432" };

describe("validateForward", () => {
  test("-L por defecto en loopback sin aviso", () => {
    const r = validateForward(base);
    expect(r).toEqual({
      ok: true,
      warning: false,
      spec: { kind: "local", bind: "127.0.0.1", port: 8080, host: "db.internal", host_port: 5432 },
    });
  });

  test("bind no loopback genera aviso", () => {
    const r = validateForward({ ...base, bind: "0.0.0.0" });
    expect(r.ok && r.warning).toBe(true);
  });

  test("-D no pide destino", () => {
    const r = validateForward({ ...base, kind: "dynamic", host: "", hostPort: "" });
    expect(r).toEqual({ ok: true, warning: false, spec: { kind: "dynamic", bind: "127.0.0.1", port: 8080 } });
  });

  test("rechaza puertos fuera de rango y no numéricos", () => {
    expect(validateForward({ ...base, port: "0" }).ok).toBe(false);
    expect(validateForward({ ...base, port: "65536" }).ok).toBe(false);
    expect(validateForward({ ...base, port: "80a" }).ok).toBe(false);
    expect(validateForward({ ...base, hostPort: "" }).ok).toBe(false);
  });

  test("rechaza host vacío o con espacios/caracteres de comando", () => {
    expect(validateForward({ ...base, host: "" }).ok).toBe(false);
    expect(validateForward({ ...base, host: "a b" }).ok).toBe(false);
    expect(validateForward({ ...base, host: "x;rm" }).ok).toBe(false);
  });

  test("rechaza bind inválido", () => {
    expect(validateForward({ ...base, bind: "bad bind" }).ok).toBe(false);
  });
});

test("isLoopbackBind", () => {
  expect(isLoopbackBind("localhost")).toBe(true);
  expect(isLoopbackBind("127.0.0.2")).toBe(true);
  expect(isLoopbackBind("::1")).toBe(true);
  expect(isLoopbackBind("192.168.1.5")).toBe(false);
});
