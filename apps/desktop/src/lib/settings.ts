import type { DesktopSettings } from "./types";

export const FONT_MIN = 8;
export const FONT_MAX = 32;
export const SCROLLBACK_MAX = 100_000;

export const DEFAULT_SETTINGS: DesktopSettings = {
  theme: "system",
  fontFamily: "",
  fontSize: 13,
  cursorStyle: "block",
  cursorBlink: true,
  scrollback: 10_000,
  copyOnSelect: false,
  rightClick: "menu",
  confirmClose: true,
  restoreQuick: false,
};

const pick = <T extends string>(v: unknown, all: readonly T[], d: T): T =>
  all.includes(v as T) ? (v as T) : d;
const num = (v: unknown, min: number, max: number, d: number): number =>
  typeof v === "number" && Number.isFinite(v) ? Math.min(max, Math.max(min, Math.round(v))) : d;
const bool = (v: unknown, d: boolean): boolean => (typeof v === "boolean" ? v : d);

/** Acepta cualquier valor (p. ej. un campo vacío del formulario) y devuelve ajustes válidos para el backend. */
export function normalizeSettings(raw: Partial<Record<keyof DesktopSettings, unknown>> | null | undefined): DesktopSettings {
  const r = raw ?? {};
  const d = DEFAULT_SETTINGS;
  const fam = typeof r.fontFamily === "string" ? r.fontFamily.replace(/\p{Cc}/gu, "").trim().slice(0, 200) : d.fontFamily;
  return {
    theme: pick(r.theme, ["system", "light", "dark"], d.theme),
    fontFamily: fam,
    fontSize: num(r.fontSize, FONT_MIN, FONT_MAX, d.fontSize),
    cursorStyle: pick(r.cursorStyle, ["block", "underline", "bar"], d.cursorStyle),
    cursorBlink: bool(r.cursorBlink, d.cursorBlink),
    scrollback: num(r.scrollback, 0, SCROLLBACK_MAX, d.scrollback),
    copyOnSelect: bool(r.copyOnSelect, d.copyOnSelect),
    rightClick: pick(r.rightClick, ["paste", "menu"], d.rightClick),
    confirmClose: bool(r.confirmClose, d.confirmClose),
    restoreQuick: bool(r.restoreQuick, d.restoreQuick),
  };
}

/** Tamaño efectivo: ajuste base + zoom temporal (Cmd/Ctrl +/-), acotado. */
export const effectiveFontSize = (base: number, zoom: number): number =>
  Math.min(FONT_MAX, Math.max(FONT_MIN, base + zoom));
