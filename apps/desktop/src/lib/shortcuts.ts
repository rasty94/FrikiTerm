// Atajos globales. macOS usa Cmd; el resto Ctrl+Shift para no pisar los de la shell (^C, ^W, ^D…).
export type ShortcutAction =
  | "newTab" | "closeTab" | "nextTab" | "prevTab" | "splitRight" | "splitDown" | "nextPane"
  | "find" | "zoomIn" | "zoomOut" | "zoomReset"
  | "view:hosts" | "view:terminal" | "view:sftp" | "view:keychain" | "view:snippets" | "view:forwards" | "view:settings";

interface Def {
  action: ShortcutAction;
  label: string;
  /** Combinación: modificadores en minúscula + `KeyboardEvent.code` (independiente del layout). */
  mac: string[];
  other: string[];
}

export const SHORTCUTS: Def[] = [
  { action: "newTab", label: "Nueva pestaña (conexión rápida)", mac: ["meta+KeyT"], other: ["ctrl+shift+KeyT"] },
  { action: "closeTab", label: "Cerrar pestaña", mac: ["meta+KeyW"], other: ["ctrl+shift+KeyW"] },
  { action: "nextTab", label: "Pestaña siguiente", mac: ["ctrl+Tab", "meta+shift+BracketRight"], other: ["ctrl+Tab", "ctrl+PageDown"] },
  { action: "prevTab", label: "Pestaña anterior", mac: ["ctrl+shift+Tab", "meta+shift+BracketLeft"], other: ["ctrl+shift+Tab", "ctrl+PageUp"] },
  { action: "splitRight", label: "Dividir a la derecha", mac: ["meta+KeyD"], other: ["ctrl+shift+KeyD"] },
  { action: "splitDown", label: "Dividir abajo", mac: ["meta+shift+KeyD"], other: ["ctrl+shift+KeyE"] },
  { action: "nextPane", label: "Siguiente panel", mac: ["meta+BracketRight"], other: ["ctrl+shift+KeyO"] },
  { action: "find", label: "Buscar en el terminal", mac: ["meta+KeyF"], other: ["ctrl+shift+KeyF"] },
  { action: "zoomIn", label: "Aumentar fuente", mac: ["meta+Equal", "meta+shift+Equal"], other: ["ctrl+shift+Equal"] },
  { action: "zoomOut", label: "Reducir fuente", mac: ["meta+Minus"], other: ["ctrl+shift+Minus"] },
  { action: "zoomReset", label: "Tamaño de fuente por defecto", mac: ["meta+Digit0"], other: ["ctrl+shift+Digit0"] },
  { action: "view:hosts", label: "Ir a Hosts", mac: ["meta+Digit1"], other: ["ctrl+shift+Digit1"] },
  { action: "view:terminal", label: "Ir a Terminal", mac: ["meta+Digit2"], other: ["ctrl+shift+Digit2"] },
  { action: "view:sftp", label: "Ir a SFTP", mac: ["meta+Digit3"], other: ["ctrl+shift+Digit3"] },
  { action: "view:keychain", label: "Ir a Llavero", mac: ["meta+Digit4"], other: ["ctrl+shift+Digit4"] },
  { action: "view:snippets", label: "Ir a Snippets", mac: ["meta+Digit5"], other: ["ctrl+shift+Digit5"] },
  { action: "view:forwards", label: "Ir a Redirecciones", mac: ["meta+Digit6"], other: ["ctrl+shift+Digit6"] },
  { action: "view:settings", label: "Ir a Ajustes", mac: ["meta+Comma"], other: ["ctrl+shift+Comma"] },
];

export interface KeyLike {
  code: string;
  metaKey: boolean;
  ctrlKey: boolean;
  shiftKey: boolean;
  altKey: boolean;
}

export const isMac = (): boolean => typeof navigator !== "undefined" && /Mac|iPhone|iPad/.test(navigator.platform);

function matches(combo: string, e: KeyLike): boolean {
  const parts = combo.split("+");
  const code = parts.pop();
  const mods = new Set(parts);
  return (
    e.code === code &&
    e.metaKey === mods.has("meta") &&
    e.ctrlKey === mods.has("ctrl") &&
    e.shiftKey === mods.has("shift") &&
    e.altKey === mods.has("alt")
  );
}

export function matchShortcut(e: KeyLike, mac: boolean): ShortcutAction | null {
  for (const d of SHORTCUTS) if ((mac ? d.mac : d.other).some((c) => matches(c, e))) return d.action;
  return null;
}

const CODE_LABEL: Record<string, string> = {
  BracketRight: "]", BracketLeft: "[", Equal: "+", Minus: "-", Comma: ",", Tab: "Tab", PageDown: "AvPág", PageUp: "RePág",
};

/** "meta+shift+KeyD" -> "⌘⇧D" (mac) o "Ctrl+Shift+D". */
export function formatCombo(combo: string, mac: boolean): string {
  const parts = combo.split("+");
  const code = parts.pop() ?? "";
  const key = CODE_LABEL[code] ?? code.replace(/^(Key|Digit)/, "");
  const mod = (m: string) => (mac ? { meta: "⌘", ctrl: "⌃", shift: "⇧", alt: "⌥" }[m] : { meta: "Win", ctrl: "Ctrl", shift: "Shift", alt: "Alt" }[m]);
  const mods = parts.map(mod);
  return mac ? mods.join("") + key : [...mods, key].join("+");
}

export const shortcutLabel = (d: Def, mac: boolean): string => formatCombo((mac ? d.mac : d.other)[0], mac);
