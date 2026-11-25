// Estado global con runes. Las pestañas viven aquí para sobrevivir a cambios de vista.
import type { Prompt, PromptId, SessionConfig, TermTarget } from "./types";

export type ViewName = "hosts" | "terminal" | "sftp" | "keychain" | "snippets" | "forwards";
export type ThemePref = "system" | "light" | "dark";

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
});
applyTheme(app.theme);

const THEME_ORDER: ThemePref[] = ["system", "light", "dark"];
export function cycleTheme(): void {
  app.theme = THEME_ORDER[(THEME_ORDER.indexOf(app.theme) + 1) % THEME_ORDER.length];
  applyTheme(app.theme);
  try {
    localStorage.setItem(THEME_KEY, app.theme);
  } catch {
    /* sin almacenamiento: el tema dura lo que la sesión */
  }
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
