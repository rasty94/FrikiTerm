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
  import Icon from "./Icon.svelte";
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
    /** Sustituye al título visible (p. ej. el selector de host del panel remoto). */
    heading?: import("svelte").Snippet;
    /** Si se da, ocupa el lugar de la tabla (estado sin conexión). */
    placeholder?: import("svelte").Snippet;
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
    heading,
    placeholder,
    onnavigate,
    onsort,
    onselect,
    ontransfer,
  }: Props = $props();

  const sorted = $derived(sortEntries(entries, sort));
  const relative = $derived(rootName !== undefined);
  const crumbs = $derived(relative ? relBreadcrumb(rootName ?? "", path) : breadcrumb(path));
  const atTop = $derived(relative ? path === "" : path === "/");


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

{#snippet th(key: SortKey, label: string, cls = "")}
  <th class={cls} aria-sort={sort.key === key ? (sort.dir === "asc" ? "ascending" : "descending") : undefined}>
    <button class="sort" class:on={sort.key === key} onclick={() => onsort(key)}>
      {label}<span class="arrow"><Icon name={sort.key === key && sort.dir === "desc" ? "chevronDown" : "chevronUp"} size={12} /></span>
    </button>
  </th>
{/snippet}

<section
  class="pane panel"
  class:drop={dropHighlight}
  bind:this={rootEl}
  ondragover={onDragOver}
  ondrop={onDrop}
  aria-label={title}
>
  <header class="panel-head">
    {#if heading}{@render heading()}{:else}<span class="title"><Icon name="monitor" size={15} />{title}</span>{/if}
    <div class="extra">{@render headerExtra?.()}</div>
  </header>

  {#if placeholder}
    {@render placeholder()}
  {:else}
    <nav class="crumbs" aria-label="Ruta">
      <button class="icon-btn up" title="Carpeta superior" aria-label="Carpeta superior" onclick={() => onnavigate(relative ? relParent(path) : parentPath(path))} disabled={atTop}><Icon name="up" size={15} /></button>
      <div class="trail">
        {#each crumbs as c, i (c.path)}
          {#if i > 0 && crumbs[i - 1].label !== "/"}<span class="sep" aria-hidden="true">/</span>{/if}
          <button class="crumb" class:last={i === crumbs.length - 1} onclick={() => onnavigate(c.path)}>{c.label}</button>
        {/each}
      </div>
    </nav>

    {#if error}
      <p class="alert"><Icon name="alert" size={15} />{error}</p>
    {/if}

    <div class="table-wrap">
      <table class="table">
        <thead>
          <tr>
            {@render th("name", "Nombre")}
            {@render th("size", "Tamaño", "num size")}
            {@render th("mtime", "Modificado", "date")}
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
              <td class="name"><div class="cellf">
                <span class="ico {entry.kind}"><Icon name={entry.kind === "dir" ? "folder" : entry.kind === "symlink" ? "link" : "file"} size={16} /></span>
                <span class="fname">{entry.name}</span>
              </div></td>
              <td class="num">{entry.kind === "dir" ? "—" : formatSize(entry.size)}</td>
              <td class="date">{formatMtime(entry.mtime)}</td>
            </tr>
          {:else}
            <tr class="none"><td colspan="3" class="empty">{loading ? "Cargando…" : error ? "" : "Carpeta vacía"}</td></tr>
          {/each}
        </tbody>
      </table>
    </div>
  {/if}
</section>

<style>
  .pane { display: flex; flex-direction: column; min-width: 0; min-height: 0; overflow: hidden; transition: box-shadow 120ms var(--ease-out); }
  .pane.drop { box-shadow: inset 0 0 0 2px var(--accent); background: var(--accent-soft); }
  .panel-head { padding: 0 8px 0 14px; gap: 10px; }
  .title { display: flex; align-items: center; gap: 8px; font-weight: 600; color: var(--fg); }
  .title :global(svg) { color: var(--fg-dim); }
  .extra { display: flex; gap: 2px; margin-left: auto; align-items: center; }
  .crumbs { display: flex; align-items: center; gap: 6px; padding: 6px 10px 6px 6px; border-bottom: 1px solid var(--border); }
  .up { width: 26px; height: 26px; }
  .trail {
    flex: 1; min-width: 0; display: flex; align-items: center; overflow-x: auto; scrollbar-width: none;
    height: 26px; padding: 0 4px; border-radius: var(--r-sm); background: var(--bg-input); border: 1px solid var(--border);
    font-family: var(--font-mono); font-size: 12px;
  }
  .crumb { height: 20px; padding: 0 2px; border: 0; background: none; border-radius: 4px; font: inherit; color: var(--fg-dim); }
  .crumb:hover:not(:disabled) { background: var(--bg-hover); color: var(--fg); }
  .crumb.last { color: var(--fg); font-weight: 600; }
  .sep { color: var(--fg-faint); padding: 0 1px; }
  .alert { margin: 8px 10px 0; }
  .table-wrap { flex: 1; overflow: auto; min-height: 0; }
  th { padding: 0; }
  th.size { width: 84px; }
  th.date { width: 150px; }
  .sort { width: 100%; height: 30px; justify-content: flex-start; gap: 3px; padding: 0 12px; border: 0; border-radius: 0; background: none; font: inherit; font-weight: 500; color: var(--fg-dim); }
  th.num .sort { justify-content: flex-end; }
  .sort:hover:not(:disabled) { background: var(--bg-hover); color: var(--fg); }
  .sort.on { color: var(--fg); }
  .arrow { display: flex; opacity: 0; }
  .sort.on .arrow { opacity: 1; }
  .sort:hover .arrow { opacity: 0.5; }
  .sort.on:hover .arrow { opacity: 1; }
  td { height: 30px; border-bottom: 0; white-space: nowrap; }
  tbody tr { cursor: default; }
  tbody tr:focus-visible { outline: 2px solid var(--accent); outline-offset: -2px; }
  tbody tr.selected, tbody tr.selected:hover { background: var(--accent-soft); }
  tbody tr.selected .fname { color: var(--accent); font-weight: 500; }
  tbody tr.none:hover { background: none; }
  td.name { max-width: 0; width: 100%; }
  .cellf { display: flex; align-items: center; gap: 9px; min-width: 0; }
  .fname { overflow: hidden; text-overflow: ellipsis; }
  .ico { display: flex; color: var(--fg-faint); }
  .ico.dir { color: var(--accent); }
  .ico.dir :global(svg) { fill: var(--accent-soft); }
  td.num { color: var(--fg-dim); font-size: 12px; }
  td.date { color: var(--fg-dim); font-size: 12px; font-variant-numeric: tabular-nums; }
  .empty { color: var(--fg-faint); text-align: center; height: 80px; }
</style>
