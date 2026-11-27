import { describe, expect, it } from "vitest";
import { DEFAULT_SETTINGS, effectiveFontSize, normalizeSettings } from "./settings";

describe("normalizeSettings", () => {
  it("devuelve los valores por defecto con entrada vacía", () => {
    expect(normalizeSettings(null)).toEqual(DEFAULT_SETTINGS);
    expect(normalizeSettings({})).toEqual(DEFAULT_SETTINGS);
  });
  it("acota rangos y redondea", () => {
    const s = normalizeSettings({ fontSize: 99, scrollback: -5 });
    expect(s.fontSize).toBe(32);
    expect(s.scrollback).toBe(0);
    expect(normalizeSettings({ fontSize: 12.6 }).fontSize).toBe(13);
  });
  it("descarta enums y tipos inválidos", () => {
    const s = normalizeSettings({ theme: "neon", cursorStyle: 3, fontSize: "x", cursorBlink: "sí" });
    expect(s).toEqual(DEFAULT_SETTINGS);
  });
  it("limpia la familia de fuente", () => {
    expect(normalizeSettings({ fontFamily: "  Fira Code\n" }).fontFamily).toBe("Fira Code");
  });
});

describe("effectiveFontSize", () => {
  it("suma el zoom y acota", () => {
    expect(effectiveFontSize(13, 2)).toBe(15);
    expect(effectiveFontSize(13, -50)).toBe(8);
    expect(effectiveFontSize(30, 10)).toBe(32);
  });
});
