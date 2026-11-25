<script lang="ts" module>
  import type { EntryKind } from "../lib/types";
  /** Tipo MIME propio para arrastrar filas entre paneles (HTML5 DnD dentro de la webview). */
  export const DRAG_MIME = "application/x-friki-entry";
  export interface DragPayload {
    side: "local" | "remote";
    name: string;
    path: string;
    kind: EntryKind;
  }
</script>

<script lang="ts">
  import type { EntryDto } from "../lib/types";
  import { breadcrumb, formatMtime, formatSize, parentPath, relBreadcrumb, relParent, sortEntries, type SortKey, type SortState } from "../lib/sftp-logic";

  interface Props {
    side: "local" | "remote";
    title: string;
    path: string;
    entries: EntryDto[];
    loading: boolean;
    error: string | null;
    sort: SortState;
    selected: string | null;
    /** Resaltado mientras se arrastra un fichero del sistema sobre este panel. */
    dropHighlight?: boolean;
    rootEl?: HTMLElement;
    /** Si se da, `path` es relativa a una raíz con este nombre ("" = la raíz). */
    rootName?: string;
    headerExtra?: import("svelte").Snippet;
    onnavigate: (path: string) => void;
    onsort: (key: SortKey) => void;
    onselect: (path: string) => void;
    ontransfer: (src: DragPayload) => void;
  }

  let {
    side,
    title,
    path,
    entries,
    loading,
    error,
    sort,
    selected,
    dropHighlight = false,
    rootEl = $bindable(),
    rootName,
    headerExtra,
    onnavigate,
    onsort,
    onselect,
    ontransfer,
  }: Props = $props();

  const sorted = $derived(sortEntries(entries, sort));
  const relative = $derived(rootName !== undefined);
  const crumbs = $derived(relative ? relBreadcrumb(rootName ?? "", path) : breadcrumb(path));
  const atTop = $derived(relative ? path === "" : path === "/");

  function arrow(key: SortKey): string {
    if (sort.key !== key) return "";
    return sort.dir === "asc" ? " ▲" : " ▼";
  }

  function onDragStart(e: DragEvent, entry: EntryDto) {
    if (!e.dataTransfer) return;
    const payload: DragPayload = { side, name: entry.name, path: entry.path, kind: entry.kind };
    e.dataTransfer.setData(DRAG_MIME, JSON.stringify(payload));
    e.dataTransfer.effectAllowed = "copy";
  }

  function hasOwnPayload(e: DragEvent): boolean {
    return !!e.dataTransfer && Array.from(e.dataTransfer.types).includes(DRAG_MIME);
  }

  function onDragOver(e: DragEvent) {
    if (!hasOwnPayload(e)) return;
    e.preventDefault();
    if (e.dataTransfer) e.dataTransfer.dropEffect = "copy";
  }

  function onDrop(e: DragEvent) {
    if (!e.dataTransfer || !hasOwnPayload(e)) return;
    e.preventDefault();
    const raw = e.dataTransfer.getData(DRAG_MIME);
    try {
      const payload = JSON.parse(raw) as DragPayload;
      if (payload.side !== side) ontransfer(payload);
    } catch {
      // Payload inválido: se ignora.
    }
  }

  function formatKind(kind: EntryKind): string {
    return kind === "dir" ? "Carpeta" : kind === "symlink" ? "Enlace" : kind === "other" ? "Otro" : "Archivo";
  }
</script>

<section
  class="pane"
  class:drop={dropHighlight}
  bind:this={rootEl}
  ondragover={onDragOver}
  ondrop={onDrop}
  aria-label={title}
>
  <header>
    <strong>{title}</strong>
    <div class="extra">{@render headerExtra?.()}</div>
  </header>
  <nav class="crumbs">
    <button class="icon" title="Subir" onclick={() => onnavigate(relative ? relParent(path) : parentPath(path))} disabled={atTop}>↑</button>
    {#each crumbs as c, i (c.path)}
      {#if i > 0}<span class="sep">/</span>{/if}
      <button class="crumb" onclick={() => onnavigate(c.path)}>{c.label}</button>
    {/each}
  </nav>

  {#if error}
    <p class="error">{error}</p>
  {/if}

  <div class="table-wrap">
    <table>
      <thead>
        <tr>
          <th><button class="sort" onclick={() => onsort("name")}>Nombre{arrow("name")}</button></th>
          <th class="num"><button class="sort" onclick={() => onsort("size")}>Tamaño{arrow("size")}</button></th>
          <th><button class="sort" onclick={() => onsort("mtime")}>Modificado{arrow("mtime")}</button></th>
        </tr>
      </thead>
      <tbody>
        {#each sorted as entry (entry.path)}
          <tr
            class:selected={selected === entry.path}
            draggable="true"
            ondragstart={(e) => onDragStart(e, entry)}
            onclick={() => onselect(entry.path)}
            ondblclick={() => entry.kind === "dir" && onnavigate(entry.path)}
            onkeydown={(e) => {
              if (e.key === "Enter" && entry.kind === "dir") onnavigate(entry.path);
            }}
            tabindex="0"
            title={formatKind(entry.kind)}
          >
            <td class="name"><span class="ico">{entry.kind === "dir" ? "▸" : " "}</span>{entry.name}</td>
            <td class="num">{entry.kind === "dir" ? "" : formatSize(entry.size)}</td>
            <td class="mono">{formatMtime(entry.mtime)}</td>
          </tr>
        {:else}
          <tr><td colspan="3" class="empty">{loading ? "Cargando…" : "Carpeta vacía"}</td></tr>
        {/each}
      </tbody>
    </table>
  </div>
</section>

<style>
  .pane {
    display: flex;
    flex-direction: column;
    min-width: 0;
    min-height: 0;
    background: var(--bg-elev);
    border: 1px solid var(--border);
    border-radius: 8px;
    overflow: hidden;
  }
  .pane.drop { outline: 2px dashed var(--accent); outline-offset: -4px; }
  header { display: flex; justify-content: space-between; align-items: center; padding: 8px 12px; border-bottom: 1px solid var(--border); }
  .extra { display: flex; gap: 6px; }
  .crumbs { display: flex; flex-wrap: wrap; align-items: center; gap: 2px; padding: 6px 8px; border-bottom: 1px solid var(--border); font-family: var(--font-mono); font-size: 12px; }
  .crumb, .icon { background: none; border: 0; color: var(--fg); cursor: pointer; padding: 2px 4px; border-radius: 4px; font: inherit; }
  .crumb:hover, .icon:hover:not(:disabled) { background: var(--border); }
  .icon:disabled { color: var(--fg-dim); cursor: default; }
  .sep { color: var(--fg-dim); }
  .error { margin: 6px 12px; color: var(--danger); }
  .table-wrap { flex: 1; overflow: auto; min-height: 0; }
  table { width: 100%; border-collapse: collapse; font-size: 13px; }
  th, td { padding: 4px 10px; text-align: left; white-space: nowrap; }
  th { position: sticky; top: 0; background: var(--bg-elev); border-bottom: 1px solid var(--border); }
  th button.sort { background: none; border: 0; color: var(--fg-dim); font: inherit; cursor: pointer; padding: 0; }
  td.num, th.num { text-align: right; }
  .mono { font-family: var(--font-mono); color: var(--fg-dim); font-size: 12px; }
  tbody tr { cursor: default; }
  tbody tr:hover { background: var(--border); }
  tbody tr.selected { background: var(--term-selection); }
  .ico { display: inline-block; width: 14px; color: var(--fg-dim); }
  td.name { max-width: 320px; overflow: hidden; text-overflow: ellipsis; }
  .empty { color: var(--fg-dim); text-align: center; padding: 16px; }
</style>
