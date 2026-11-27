import { describe, expect, it } from "vitest";
import { formatCombo, matchShortcut, SHORTCUTS } from "./shortcuts";

const k = (code: string, m: Partial<{ metaKey: boolean; ctrlKey: boolean; shiftKey: boolean; altKey: boolean }> = {}) => ({
  code, metaKey: false, ctrlKey: false, shiftKey: false, altKey: false, ...m,
});

describe("matchShortcut", () => {
  it("mac usa Cmd", () => {
    expect(matchShortcut(k("KeyT", { metaKey: true }), true)).toBe("newTab");
    expect(matchShortcut(k("KeyD", { metaKey: true, shiftKey: true }), true)).toBe("splitDown");
    expect(matchShortcut(k("Digit1", { metaKey: true }), true)).toBe("view:hosts");
  });
  it("otros usan Ctrl+Shift y no pisan Ctrl+letra de la shell", () => {
    expect(matchShortcut(k("KeyT", { ctrlKey: true, shiftKey: true }), false)).toBe("newTab");
    expect(matchShortcut(k("KeyW", { ctrlKey: true }), false)).toBeNull();
    expect(matchShortcut(k("KeyD", { ctrlKey: true }), false)).toBeNull();
    expect(matchShortcut(k("KeyC", { ctrlKey: true }), false)).toBeNull();
  });
  it("exige el conjunto exacto de modificadores", () => {
    expect(matchShortcut(k("KeyT", { metaKey: true, altKey: true }), true)).toBeNull();
    expect(matchShortcut(k("Tab", { ctrlKey: true }), true)).toBe("nextTab");
    expect(matchShortcut(k("Tab", { ctrlKey: true, shiftKey: true }), false)).toBe("prevTab");
  });
  it("no hay combinaciones repetidas en una misma plataforma", () => {
    for (const p of ["mac", "other"] as const) {
      const all = SHORTCUTS.flatMap((d) => d[p]);
      expect(new Set(all).size).toBe(all.length);
    }
  });
});

describe("formatCombo", () => {
  it("formatea por plataforma", () => {
    expect(formatCombo("meta+shift+KeyD", true)).toBe("⌘⇧D");
    expect(formatCombo("ctrl+shift+Digit1", false)).toBe("Ctrl+Shift+1");
  });
});
