// Estado global con runes. Las pestañas viven aquí para sobrevivir a cambios de vista.
import type { DesktopSettings, Prompt, PromptId, SessionConfig, TermTarget, ThemePref } from "./types";
import { settingsGet, settingsSave } from "./api";
import { DEFAULT_SETTINGS, normalizeSettings } from "./settings";
import { parseWorkspace, pushHistory, reorder, serializeNode, serializeWorkspace, type SavedNode, type SavedWorkspace } from "./layout";
import { defaultSession } from "./session";

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
  /** Ruta del registro de sesión activo (REC); null si no se graba. */
  log: string | null;
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
  /** "Enviar a todos": la entrada de un panel se refleja en los demás paneles abiertos de la pestaña. */
  broadcast: boolean;
}

export interface Toast {
  id: number;
  kind: "error" | "info";
  text: string;
}

/** Niveles máximos de división anidada. */
const MAX_DEPTH = 2;
const THEME_KEY = "friki.theme";
const WS_KEY = "friki.workspace.v1";
const QUICK_KEY = "friki.quickHistory";

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
  /** Espacio de trabajo guardado de la sesión anterior, pendiente de que el usuario lo restaure. */
  restore: loadWorkspace(),
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
  return { kind: "leaf", id: nextId(), title, target, status: { state: "connecting" }, log: null };
}

/** Escritores de los paneles con sesión abierta (hoja -> escribir bytes), para la difusión de entrada. */
export const paneWriters = new Map<number, (data: Uint8Array) => void>();

export const tabById = (id: number) => app.tabs.find((t) => t.id === id);

export function openTab(target: TermTarget, title: string): void {
  const root = newLeaf(target, title);
  addTab(root, title, false);
}

function addTab(root: PaneNode, title: string, broadcast: boolean, at?: number): Tab {
  const tab: Tab = { id: nextId(), title, root, activeLeaf: firstLeaf(root).id, broadcast };
  app.tabs.splice(at ?? app.tabs.length, 0, tab);
  app.activeTab = tab.id;
  app.view = "terminal";
  persistWorkspace();
  return tab;
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
  persistWorkspace();
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
  persistWorkspace();
}

export function closePane(tabId: number, leafId: number): void {
  const tab = tabById(tabId);
  if (!tab) return;
  const next = removeLeaf(tab.root, leafId);
  if (!next) return closeTab(tabId);
  tab.root = next;
  if (tab.activeLeaf === leafId) tab.activeLeaf = firstLeaf(next).id;
  persistWorkspace();
}

// ---- Pestañas: renombrar, reordenar, duplicar, cerrar otras, difusión ----

export function renameTab(tabId: number, title: string): void {
  const tab = tabById(tabId);
  const t = title.replace(/\p{Cc}/gu, "").trim().slice(0, 60);
  if (tab && t) {
    tab.title = t;
    persistWorkspace();
  }
}

export function moveTab(from: number, to: number): void {
  app.tabs = reorder(app.tabs, from, to);
  persistWorkspace();
}

function buildNode(n: SavedNode): PaneNode {
  if (n.k === "leaf") {
    const t = n.target;
    const target: TermTarget =
      t.kind === "quick" ? { kind: "quick", spec: defaultSession({ host: t.host, port: t.port, user: t.user }) } : t;
    return newLeaf(target, n.title);
  }
  return { kind: "split", id: nextId(), dir: n.dir, children: n.children.map(buildNode) };
}

/** Pestaña nueva a la derecha de la original con el mismo diseño y destinos (sesiones nuevas). */
export function duplicateTab(tabId: number): void {
  const i = app.tabs.findIndex((t) => t.id === tabId);
  const src = app.tabs[i];
  const node = src && serializeNode(src.root, true);
  if (node) addTab(buildNode(node), src.title, src.broadcast, i + 1);
}

/** Cierra las demás pestañas (con confirmación si alguna tiene sesión viva). */
export async function closeOtherTabs(tabId: number): Promise<void> {
  const others = app.tabs.filter((t) => t.id !== tabId);
  const live = others.some((t) => leaves(t.root).some((l) => l.status.state === "open" || l.status.state === "connecting"));
  if (live && app.settings.confirmClose && !(await askConfirm(`¿Cerrar las otras ${others.length} pestañas? Sus sesiones activas se cerrarán.`, "Cerrar"))) return;
  app.tabs = app.tabs.filter((t) => t.id === tabId);
  app.activeTab = tabId;
  persistWorkspace();
}

export function toggleBroadcast(tabId: number): void {
  const tab = tabById(tabId);
  if (!tab) return;
  tab.broadcast = !tab.broadcast;
  persistWorkspace();
  toast(tab.broadcast ? "Enviar a todos activado: lo que escribas llega a todos los paneles de la pestaña" : "Enviar a todos desactivado", "info");
}

// ---- Persistencia (localStorage, siempre con try/catch) ----

function loadWorkspace(): SavedWorkspace | null {
  try {
    return parseWorkspace(JSON.parse(localStorage.getItem(WS_KEY) ?? "null"));
  } catch {
    return null;
  }
}

/** Guarda pestañas, diseño y destinos. Mientras haya una restauración pendiente y ninguna pestaña, no se pisa. */
export function persistWorkspace(): void {
  if (app.restore && app.tabs.length === 0) return;
  try {
    const ws = serializeWorkspace(app.tabs, app.activeTab, app.settings.restoreQuick);
    if (ws) localStorage.setItem(WS_KEY, JSON.stringify(ws));
    else localStorage.removeItem(WS_KEY);
  } catch {
    /* sin almacenamiento: no se restaura */
  }
}

export function restoreWorkspace(): void {
  const ws = app.restore;
  app.restore = null;
  if (!ws) return;
  const ids = ws.tabs.map((t) => addTab(buildNode(t.root), t.title, t.broadcast).id);
  app.activeTab = ids[ws.active] ?? ids[0];
}

export function dismissRestore(): void {
  app.restore = null;
  persistWorkspace();
}

export function loadQuickHistory(): string[] {
  try {
    const v = JSON.parse(localStorage.getItem(QUICK_KEY) ?? "[]");
    return Array.isArray(v) ? v.filter((x): x is string => typeof x === "string").slice(0, 10) : [];
  } catch {
    return [];
  }
}

export function rememberQuick(spec: string): void {
  try {
    localStorage.setItem(QUICK_KEY, JSON.stringify(pushHistory(loadQuickHistory(), spec)));
  } catch {
    /* solo es comodidad */
  }
}

if (typeof window !== "undefined") {
  window.addEventListener("pagehide", persistWorkspace);
  window.addEventListener("beforeunload", persistWorkspace);
  setInterval(persistWorkspace, 15_000);
  // Con una sesión anterior guardada se ofrece restaurarla en la vista Terminal.
  if (app.restore) app.view = "terminal";
}
