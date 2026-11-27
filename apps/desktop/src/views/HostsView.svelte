<script lang="ts">
  import { onMount } from "svelte";
  import { hostDelete, hostSave, hostsImportSshConfig, hostsList, hostsMeta } from "../lib/api";
  import type { SessionConfig } from "../lib/types";
  import { navTarget, recentHosts, sortHosts, type MetaMap, type SortMode, type ViewMode } from "../lib/hostMeta";
  import { app, askConfirm, openTab, toast } from "../lib/state.svelte";
  import { errMsg } from "../lib/session";
  import { buildFolderTree, hostsInFolder } from "../lib/folderTree";
  import { filterHosts } from "../lib/search";
  import FolderTree from "../components/FolderTree.svelte";
  import HostRow from "../components/HostRow.svelte";
  import HostCard from "../components/HostCard.svelte";
  import HostForm from "../components/HostForm.svelte";
  import SearchBox from "../components/SearchBox.svelte";
  import ContextMenu from "../components/ContextMenu.svelte";
  import Icon from "../components/Icon.svelte";

  let folder = $state<string | null>(null);
  let query = $state("");
  let editor = $state<{ host: SessionConfig | null } | null>(null);
  let menu = $state<{ x: number; y: number; host: SessionConfig } | null>(null);
  let loaded = $state(false);
  let meta = $state<MetaMap>({});
  let searchEl = $state<HTMLInputElement>();
  let itemsEl = $state<HTMLElement>();

  // Preferencias de vista: por usuario, en localStorage (puede no estar disponible).
  function pref<T extends string>(key: string, ok: readonly T[], def: T): T {
    try {
      const v = localStorage.getItem(key) as T | null;
      return v && ok.includes(v) ? v : def;
    } catch {
      return def;
    }
  }
  let view = $state<ViewMode>(pref("friki.hostsView", ["list", "grid"], "list"));
  let sort = $state<SortMode>(pref("friki.hostsSort", ["name", "recent"], "name"));
  $effect(() => {
    try {
      localStorage.setItem("friki.hostsView", view);
      localStorage.setItem("friki.hostsSort", sort);
    } catch {
      /* sin almacenamiento: la preferencia vale solo esta sesión */
    }
  });

  const tree = $derived(buildFolderTree(app.hosts));
  const folders = $derived(
    [...new Set(app.hosts.map((h) => h.folder).filter(Boolean))].sort(),
  );
  const visible = $derived(sortHosts(filterHosts(hostsInFolder(app.hosts, folder), query), meta, sort));
  // "Recientes" solo en la vista completa y sin filtrar: es un atajo, no un resultado de búsqueda.
  const recents = $derived(folder === null && !query.trim() ? recentHosts(app.hosts, meta) : []);

  async function refresh() {
    try {
      app.hosts = await hostsList();
    } catch (e) {
      toast(errMsg(e));
    }
    try {
      meta = await hostsMeta();
    } catch {
      /* los metadatos son opcionales */
    }
    loaded = true;
  }

  const connect = (h: SessionConfig) => openTab({ kind: "saved", name: h.name }, h.name);

  function onkeydown(e: KeyboardEvent) {
    const t = e.target as HTMLElement | null;
    const typing = !!t && (t.matches("input, textarea, select") || t.isContentEditable);
    if (e.key === "/" && !typing && !e.metaKey && !e.ctrlKey && !e.altKey) {
      e.preventDefault();
      searchEl?.focus();
      return;
    }
    if (editor || menu || !itemsEl) return;
    const items = [...itemsEl.querySelectorAll<HTMLElement>("[data-nav]")];
    const i = items.indexOf(document.activeElement as HTMLElement);
    if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "e" && i >= 0) {
      e.preventDefault();
      const h = app.hosts.find((x) => x.name === items[i].dataset.name);
      if (h) editor = { host: h };
      return;
    }
    const arrow = ["ArrowDown", "ArrowUp", "Home", "End", ...(view === "grid" ? ["ArrowLeft", "ArrowRight"] : [])];
    // Desde el buscador, ↓ salta al primer host.
    const fromSearch = t === searchEl && e.key === "ArrowDown";
    if (!arrow.includes(e.key) || e.metaKey || e.ctrlKey || e.altKey || (typing && !fromSearch)) return;
    const rects = items.map((el) => {
      const r = el.getBoundingClientRect();
      return { left: r.left, top: r.top };
    });
    const to = navTarget(e.key, fromSearch ? -1 : i, rects, view === "grid");
    if (to >= 0) {
      e.preventDefault();
      items[to].focus();
      items[to].scrollIntoView?.({ block: "nearest" });
    }
  }

  function openSftp(h: SessionConfig) {
    app.sftpHost = h.name;
    app.view = "sftp";
  }

  async function duplicate(h: SessionConfig) {
    const names = new Set(app.hosts.map((x) => x.name));
    let name = `${h.name} (copia)`;
    for (let i = 2; names.has(name); i++) name = `${h.name} (copia ${i})`;
    try {
      await hostSave({ ...h, name }, null);
      await refresh();
    } catch (e) {
      toast(errMsg(e));
    }
  }

  async function remove(h: SessionConfig) {
    if (!(await askConfirm(`¿Borrar el host «${h.name}»?`, "Eliminar"))) return;
    try {
      await hostDelete(h.name);
      editor = null;
      await refresh();
    } catch (e) {
      toast(errMsg(e));
    }
  }

  async function importSsh() {
    try {
      const r = await hostsImportSshConfig();
      toast(`Importados ${r.added} hosts`, "info");
      if (r.warnings.length) toast(r.warnings.join(" · "), "info");
      await refresh();
    } catch (e) {
      toast(errMsg(e));
    }
  }

  onMount(() => {
    refresh();
  });
</script>

<svelte:window {onkeydown} />

<div class="hosts">
  <aside class="folders" aria-label="Carpetas">
    <button class="all" class:on={folder === null} aria-pressed={folder === null} onclick={() => (folder = null)}>
      <Icon name="layers" size={15} />
      <span class="lbl">Todos los hosts</span>
      <span class="count">{app.hosts.length}</span>
    </button>
    {#if tree.children.length}
      <h2 class="group">Carpetas</h2>
      <FolderTree nodes={tree.children} selected={folder} onselect={(p) => (folder = p)} />
    {/if}
  </aside>

  {#if loaded && app.hosts.length === 0}
    <section class="empty-state first">
      <span class="ico"><Icon name="server" size={22} /></span>
      <h2>Bienvenido a FrikiTerm</h2>
      <p>Aún no tienes hosts guardados. Importa los de tu <code>~/.ssh/config</code> o crea uno a mano.</p>
      <div class="cta">
        <button class="primary" onclick={importSsh}><Icon name="download" />Importar ~/.ssh/config</button>
        <button onclick={() => (editor = { host: null })}><Icon name="plus" />Nuevo host</button>
      </div>
    </section>
  {:else}
  <section class="list">
    <header class="view-head">
      <div class="titles">
        <h1>{folder ?? "Hosts"}</h1>
        <span class="sub">{visible.length} {visible.length === 1 ? "host" : "hosts"}</span>
      </div>
      <SearchBox bind:value={query} bind:input={searchEl} />
      <div class="actions">
        <select class="sort" bind:value={sort} aria-label="Ordenar hosts">
          <option value="name">Nombre</option>
          <option value="recent">Uso reciente</option>
        </select>
        <div class="seg" role="group" aria-label="Vista">
          <button class="icon-btn" class:on={view === "list"} aria-pressed={view === "list"} aria-label="Vista de lista" title="Lista" onclick={() => (view = "list")}><Icon name="list" size={15} /></button>
          <button class="icon-btn" class:on={view === "grid"} aria-pressed={view === "grid"} aria-label="Vista de cuadrícula" title="Cuadrícula" onclick={() => (view = "grid")}><Icon name="grid" size={15} /></button>
        </div>
        <button class="ghost" onclick={importSsh} title="Importar hosts de ~/.ssh/config"><Icon name="download" />Importar</button>
        <button class="primary" onclick={() => (editor = { host: null })}><Icon name="plus" />Nuevo host</button>
      </div>
    </header>
    <div class="items" bind:this={itemsEl}>
      {#snippet entry(h: SessionConfig, title?: string)}
        {#if view === "grid"}
          <HostCard host={h} {title} meta={meta[h.name]} onconnect={() => connect(h)} oncontext={(e) => (menu = { x: e.clientX, y: e.clientY, host: h })} />
        {:else}
          <HostRow
            host={h}
            {title}
            meta={meta[h.name]}
            onconnect={() => connect(h)}
            onedit={() => (editor = { host: h })}
            oncontext={(e) => (menu = { x: e.clientX, y: e.clientY, host: h })}
          />
        {/if}
      {/snippet}
      {#if recents.length}
        <h2 class="group">Recientes</h2>
        <ul class:grid={view === "grid"} aria-label="Recientes" data-section="recents">
          {#each recents as h (h.name)}{@render entry(h, `Reciente: ${h.name}`)}{/each}
        </ul>
        <h2 class="group">Todos los hosts</h2>
      {/if}
      <ul class:grid={view === "grid"} aria-label="Hosts" data-section="all">
        {#each visible as h (h.name)}{@render entry(h)}
        {:else}
          <li class="empty-state">
            <span class="ico"><Icon name="search" size={20} /></span>
            <h3>{app.hosts.length ? "Sin resultados" : "Carpeta vacía"}</h3>
            <p>{query ? `Ningún host coincide con «${query}».` : "No hay hosts en esta carpeta."}</p>
          </li>
        {/each}
      </ul>
    </div>
  </section>
  {/if}

  {#if menu}
    {@const h = menu.host}
    <ContextMenu
      x={menu.x}
      y={menu.y}
      onclose={() => (menu = null)}
      items={[
        { label: "Conectar", run: () => connect(h) },
        ...(h.protocol === "ssh" ? [{ label: "Abrir SFTP", run: () => openSftp(h) }] : []),
        { label: "Duplicar", run: () => void duplicate(h) },
        { label: "Editar", run: () => (editor = { host: h }) },
        { label: "Eliminar", danger: true, run: () => void remove(h) },
      ]}
    />
  {/if}

  {#if editor}
    <HostForm
      host={editor.host}
      {folders}
      onclose={() => (editor = null)}
      onsaved={async () => {
        editor = null;
        await refresh();
      }}
      ondelete={remove}
    />
  {/if}
</div>

<style>
  .hosts { flex: 1; min-height: 0; display: grid; grid-template-columns: 208px minmax(0, 1fr); gap: 28px; }
  .folders { overflow: auto; display: grid; gap: 1px; align-content: start; }
  .all { width: 100%; height: 30px; justify-content: flex-start; gap: 9px; border: 0; background: none; padding: 0 10px; color: var(--fg-dim); font-weight: 500; }
  .all .lbl { flex: 1; text-align: left; color: var(--fg); }
  .all.on, .all.on:hover:not(:disabled) { background: var(--bg-active); color: var(--accent); }
  .group { font-size: 11.5px; font-weight: 500; color: var(--fg-faint); margin: 18px 10px 6px; }
  .count { color: var(--fg-faint); font-size: 11.5px; font-variant-numeric: tabular-nums; }
  .list { display: flex; flex-direction: column; gap: var(--sp-4); min-width: 0; min-height: 0; }
  .items { overflow: auto; margin: 0 -10px; padding: 0 10px 0 10px; min-height: 0; }
  .items .group { margin: 12px 0 6px; }
  .items .group:first-child { margin-top: 0; }
  ul { list-style: none; margin: 0 -10px; padding: 0; display: grid; gap: 2px; align-content: start; }
  ul.grid { margin: 0; gap: var(--sp-3); grid-template-columns: repeat(auto-fill, minmax(196px, 1fr)); padding: 2px; }
  .sort { width: auto; height: var(--ctl-h); }
  .seg { display: inline-flex; padding: 2px; gap: 2px; border: 1px solid var(--border); border-radius: var(--r); background: var(--bg-sunken); }
  .seg button { border: 0; width: 26px; height: 24px; }
  .seg button.on { background: var(--bg-elev); color: var(--fg); box-shadow: var(--shadow-1); }
  .first { grid-column: 2; }
</style>
