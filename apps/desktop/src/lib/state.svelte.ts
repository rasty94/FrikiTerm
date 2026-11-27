// Estado global con runes. Las pestañas viven aquí para sobrevivir a cambios de vista.
import type { DesktopSettings, Prompt, PromptId, SessionConfig, TermTarget, ThemePref } from "./types";
import { settingsGet, settingsSave } from "./api";
import { DEFAULT_SETTINGS, normalizeSettings } from "./settings";

export type ViewName = "hosts" | "terminal" | "sftp" | "keychain" | "snippets" | "forwards" | "settings";
export type { ThemePref };

export interface PendingPrompt {
  id: PromptId;
  prompt: Prompt;
}

export type PaneStatus =
  | { state: "connecting" }
  | { state: "open"; negotiated: string | null; warnings: string[] }
  | { state: "closed" | "failed"; text: string };

export interface Leaf {
  kind: "leaf";
  id: number;
  title: string;
  target: TermTarget;
  status: PaneStatus;
}
export interface SplitNode {
  kind: "split";
  id: number;
  dir: "row" | "col";
  children: PaneNode[];
}
export type PaneNode = Leaf | SplitNode;

export interface Tab {
  id: number;
  title: string;
  root: PaneNode;
  activeLeaf: number;
}

export interface Toast {
  id: number;
  kind: "error" | "info";
  text: string;
}

/** Niveles máximos de división anidada. */
const MAX_DEPTH = 2;
const THEME_KEY = "friki.theme";

let seq = 0;
const nextId = () => ++seq;

function loadTheme(): ThemePref {
  try {
    const v = localStorage.getItem(THEME_KEY);
    return v === "light" || v === "dark" ? v : "system";
  } catch {
    return "system";
  }
}

/** Sin atributo sigue prefers-color-scheme; con atributo lo fuerza (theme.css). */
export function applyTheme(t: ThemePref): void {
  const el = document.documentElement;
  if (t === "system") delete el.dataset.theme;
  else el.dataset.theme = t;
}

export const app = $state({
  view: "hosts" as ViewName,
  theme: loadTheme(),
  hosts: [] as SessionConfig[],
  /** Cola FIFO: el modal muestra el primero. */
  prompts: [] as PendingPrompt[],
  tabs: [] as Tab[],
  activeTab: 0,
  toasts: [] as Toast[],
  settings: { ...DEFAULT_SETTINGS } as DesktopSettings,
  /** Zoom temporal del terminal (pasos de fuente); no se guarda. */
  zoom: 0,
  /** Petición de abrir la barra de búsqueda en un panel. */
  find: { leaf: 0, n: 0 },
  /** Contador: cada incremento abre y enfoca la conexión rápida de la barra de pestañas. */
  quick: 0,
  /** Confirmación modal pendiente (ConfirmModal). */
  confirm: null as { text: string; ok: string; resolve: (v: boolean) => void } | null,
  /** Host a conectar por SFTP al entrar en la vista (lo consume SftpView). */
  sftpHost: null as string | null,
});
applyTheme(app.theme);

function cacheTheme(t: ThemePref): void {
  try {
    localStorage.setItem(THEME_KEY, t);
  } catch {
    /* sin almacenamiento: solo es una caché anti-parpadeo */
  }
}

/** Carga los ajustes del backend (el fichero manda; localStorage solo evita el parpadeo del tema). */
export async function loadSettings(): Promise<void> {
  try {
    app.settings = normalizeSettings(await settingsGet());
    app.theme = app.settings.theme;
    applyTheme(app.theme);
    cacheTheme(app.theme);
  } catch {
    /* fuera de Tauri o error de lectura: valores por defecto */
  }
}

export async function updateSettings(patch: Partial<DesktopSettings>): Promise<void> {
  const next = normalizeSettings({ ...app.settings, ...patch });
  app.settings = next;
  app.theme = next.theme;
  applyTheme(next.theme);
  cacheTheme(next.theme);
  try {
    await settingsSave(next);
  } catch (e) {
    toast(`No se pudieron guardar los ajustes: ${(e as { message?: string })?.message ?? e}`);
  }
}

/** Modal propio: `window.confirm` no funciona sin permisos del plugin de diálogo. */
export function askConfirm(text: string, ok = "Aceptar"): Promise<boolean> {
  app.confirm?.resolve(false);
  return new Promise((resolve) => {
    app.confirm = { text, ok, resolve };
  });
}

export function toast(text: string, kind: Toast["kind"] = "error"): void {
  const id = nextId();
  app.toasts.push({ id, kind, text });
  setTimeout(() => {
    const i = app.toasts.findIndex((t) => t.id === id);
    if (i >= 0) app.toasts.splice(i, 1);
  }, 5000);
}

function newLeaf(target: TermTarget, title: string): Leaf {
  return { kind: "leaf", id: nextId(), title, target, status: { state: "connecting" } };
}

export const tabById = (id: number) => app.tabs.find((t) => t.id === id);

export function openTab(target: TermTarget, title: string): void {
  const root = newLeaf(target, title);
  const tab: Tab = { id: nextId(), title, root, activeLeaf: root.id };
  app.tabs.push(tab);
  app.activeTab = tab.id;
  app.view = "terminal";
}

export function leaves(n: PaneNode): Leaf[] {
  return n.kind === "leaf" ? [n] : n.children.flatMap(leaves);
}

/** Pide confirmación (si está activada) cuando alguna sesión de la pestaña sigue viva. */
export async function requestCloseTab(tabId: number): Promise<void> {
  const tab = tabById(tabId);
  if (!tab) return;
  const live = leaves(tab.root).some((l) => l.status.state === "open" || l.status.state === "connecting");
  if (live && app.settings.confirmClose && !(await askConfirm(`¿Cerrar «${tab.title}»? La sesión activa se cerrará.`, "Cerrar"))) return;
  closeTab(tabId);
}

export function cycleTab(delta: 1 | -1): void {
  if (app.tabs.length === 0) return;
  const i = app.tabs.findIndex((t) => t.id === app.activeTab);
  app.activeTab = app.tabs[(i + delta + app.tabs.length) % app.tabs.length].id;
  app.view = "terminal";
}

export function focusNextPane(): void {
  const tab = tabById(app.activeTab);
  if (!tab) return;
  const ls = leaves(tab.root);
  const i = ls.findIndex((l) => l.id === tab.activeLeaf);
  tab.activeLeaf = ls[(i + 1) % ls.length].id;
}

export function closeTab(tabId: number): void {
  const i = app.tabs.findIndex((t) => t.id === tabId);
  if (i < 0) return;
  app.tabs.splice(i, 1);
  if (app.activeTab === tabId) app.activeTab = app.tabs[Math.max(0, i - 1)]?.id ?? 0;
}

export function setActiveLeaf(tabId: number, leafId: number): void {
  const t = tabById(tabId);
  if (t && t.activeLeaf !== leafId) t.activeLeaf = leafId;
}

export function firstLeaf(n: PaneNode): Leaf {
  return n.kind === "leaf" ? n : firstLeaf(n.children[0]);
}

function findLeaf(n: PaneNode, id: number): Leaf | null {
  if (n.kind === "leaf") return n.id === id ? n : null;
  for (const c of n.children) {
    const r = findLeaf(c, id);
    if (r) return r;
  }
  return null;
}

function depthOf(n: PaneNode, id: number, d = 0): number | null {
  if (n.kind === "leaf") return n.id === id ? d : null;
  for (const c of n.children) {
    const r = depthOf(c, id, d + 1);
    if (r !== null) return r;
  }
  return null;
}

/** Sustituye la hoja `id` por `repl`; devuelve la nueva raíz. */
function replaceLeaf(n: PaneNode, id: number, repl: PaneNode): PaneNode {
  if (n.kind === "leaf") return n.id === id ? repl : n;
  n.children = n.children.map((c) => replaceLeaf(c, id, repl));
  return n;
}

/** Quita la hoja `id`; un split que queda con un hijo colapsa. null si no quedan hojas. */
function removeLeaf(n: PaneNode, id: number): PaneNode | null {
  if (n.kind === "leaf") return n.id === id ? null : n;
  const kids = n.children.map((c) => removeLeaf(c, id)).filter((c): c is PaneNode => c !== null);
  if (kids.length === 0) return null;
  if (kids.length === 1) return kids[0];
  n.children = kids;
  return n;
}

export function splitPane(tabId: number, leafId: number, dir: "row" | "col"): void {
  const tab = tabById(tabId);
  if (!tab) return;
  const depth = depthOf(tab.root, leafId);
  if (depth === null) return;
  if (depth >= MAX_DEPTH) {
    toast("Solo se admiten 2 niveles de división", "info");
    return;
  }
  const src = findLeaf(tab.root, leafId)!;
  const fresh = newLeaf(src.target, src.title);
  tab.root = replaceLeaf(tab.root, leafId, { kind: "split", id: nextId(), dir, children: [src, fresh] });
  tab.activeLeaf = fresh.id;
}

export function closePane(tabId: number, leafId: number): void {
  const tab = tabById(tabId);
  if (!tab) return;
  const next = removeLeaf(tab.root, leafId);
  if (!next) return closeTab(tabId);
  tab.root = next;
  if (tab.activeLeaf === leafId) tab.activeLeaf = firstLeaf(next).id;
}
