// Lógica pura del explorador SFTP/local: orden, rutas y formato. Sin dependencias de Svelte ni Tauri.
import type { EntryDto } from "./types";

export type SortKey = "name" | "size" | "mtime";
export interface SortState {
  key: SortKey;
  dir: "asc" | "desc";
}

/** Carpetas primero siempre; el orden de columna se aplica dentro de cada grupo. */
export function sortEntries(entries: EntryDto[], s: SortState): EntryDto[] {
  const sign = s.dir === "asc" ? 1 : -1;
  return [...entries].sort((a, b) => {
    const group = (a.kind === "dir" ? 0 : 1) - (b.kind === "dir" ? 0 : 1);
    if (group !== 0) return group;
    let c: number;
    if (s.key === "size") c = a.size - b.size;
    else if (s.key === "mtime") c = (a.mtime ?? 0) - (b.mtime ?? 0);
    else c = a.name.localeCompare(b.name, undefined, { numeric: true, sensitivity: "base" });
    return c * sign || a.name.localeCompare(b.name);
  });
}

/** Une una ruta base con un nombre usando "/" (sirve para rutas remotas y locales POSIX). */
export function joinPath(base: string, name: string): string {
  if (base === "") return name;
  return `${base.endsWith("/") ? base : `${base}/`}${name}`;
}

/** Padre de una ruta; el padre de la raíz es la raíz. */
export function parentPath(path: string): string {
  const trimmed = path.replace(/\/+$/, "");
  const idx = trimmed.lastIndexOf("/");
  if (idx <= 0) return "/";
  return trimmed.slice(0, idx);
}

export interface Crumb {
  label: string;
  path: string;
}

/** Migas de pan desde la raíz hasta `path`. */
export function breadcrumb(path: string): Crumb[] {
  const crumbs: Crumb[] = [{ label: "/", path: "/" }];
  let acc = "";
  for (const seg of path.split("/").filter((s) => s !== "")) {
    acc = `${acc}/${seg}`;
    crumbs.push({ label: seg, path: acc });
  }
  return crumbs;
}

const UNITS = ["B", "KB", "MB", "GB", "TB"];

/** 1536 -> "1.5 KB". Base 1024. */
export function formatSize(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes < 0) return "—";
  let v = bytes;
  let i = 0;
  while (v >= 1024 && i < UNITS.length - 1) {
    v /= 1024;
    i++;
  }
  return i === 0 ? `${v} B` : `${v.toFixed(1)} ${UNITS[i]}`;
}

/** mtime llega en segundos Unix (null si el servidor no lo da). */
export function formatMtime(secs: number | null): string {
  if (secs === null) return "—";
  return new Date(secs * 1000).toLocaleString();
}
