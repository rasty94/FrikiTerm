import type { SessionConfig } from "./types";

export interface FolderNode {
  name: string;
  path: string;
  /** Hosts en esta carpeta y sus subcarpetas. */
  count: number;
  children: FolderNode[];
}

/** "/a//b/ " -> "a/b"; cadena vacía = raíz. */
export function normFolder(folder: string): string {
  return folder.split("/").map((s) => s.trim()).filter(Boolean).join("/");
}

/** Árbol de carpetas a partir del campo `folder` de cada host. La raíz no tiene nombre. */
export function buildFolderTree(hosts: SessionConfig[]): FolderNode {
  const root: FolderNode = { name: "", path: "", count: hosts.length, children: [] };
  for (const h of hosts) {
    let node = root;
    let path = "";
    for (const part of normFolder(h.folder).split("/").filter(Boolean)) {
      path = path ? `${path}/${part}` : part;
      let child = node.children.find((c) => c.name === part);
      if (!child) {
        child = { name: part, path, count: 0, children: [] };
        node.children.push(child);
      }
      child.count++;
      node = child;
    }
  }
  sortTree(root);
  return root;
}

function sortTree(n: FolderNode): void {
  n.children.sort((a, b) => a.name.localeCompare(b.name));
  n.children.forEach(sortTree);
}

/** Hosts de la carpeta `path` y sus subcarpetas; null = todos. */
export function hostsInFolder(hosts: SessionConfig[], path: string | null): SessionConfig[] {
  if (path === null) return hosts;
  return hosts.filter((h) => {
    const f = normFolder(h.folder);
    return f === path || f.startsWith(`${path}/`);
  });
}

// Tonos OKLCH escogidos para distinguirse entre sí en ambos temas.
const HUES = [255, 165, 300, 25, 75, 140, 215, 340];

/** Tono estable para el avatar de una carpeta o host: mismo nombre, mismo color. */
export function hueOf(key: string): number {
  let h = 0;
  for (const c of key) h = (h * 31 + c.codePointAt(0)!) >>> 0;
  return HUES[h % HUES.length];
}
