<script lang="ts">
  import { onMount } from "svelte";
  import { hostDelete, hostSave, hostsImportSshConfig, hostsList } from "../lib/api";
  import type { SessionConfig } from "../lib/types";
  import { app, askConfirm, openTab, toast } from "../lib/state.svelte";
  import { errMsg } from "../lib/session";
  import { buildFolderTree, hostsInFolder } from "../lib/folderTree";
  import { filterHosts } from "../lib/search";
  import FolderTree from "../components/FolderTree.svelte";
  import HostRow from "../components/HostRow.svelte";
  import HostForm from "../components/HostForm.svelte";
  import SearchBox from "../components/SearchBox.svelte";
  import ContextMenu from "../components/ContextMenu.svelte";
  import Icon from "../components/Icon.svelte";

  let folder = $state<string | null>(null);
  let query = $state("");
  let editor = $state<{ host: SessionConfig | null } | null>(null);
  let menu = $state<{ x: number; y: number; host: SessionConfig } | null>(null);
  let loaded = $state(false);

  const tree = $derived(buildFolderTree(app.hosts));
  const folders = $derived(
    [...new Set(app.hosts.map((h) => h.folder).filter(Boolean))].sort(),
  );
  const visible = $derived(
    filterHosts(hostsInFolder(app.hosts, folder), query).sort((a, b) => a.name.localeCompare(b.name)),
  );

  async function refresh() {
    try {
      app.hosts = await hostsList();
    } catch (e) {
      toast(errMsg(e));
    }
    loaded = true;
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
      <SearchBox bind:value={query} />
      <div class="actions">
        <button class="ghost" onclick={importSsh} title="Importar hosts de ~/.ssh/config"><Icon name="download" />Importar</button>
        <button class="primary" onclick={() => (editor = { host: null })}><Icon name="plus" />Nuevo host</button>
      </div>
    </header>
    <ul>
      {#each visible as h (h.name)}
        <HostRow
          host={h}
          onconnect={() => openTab({ kind: "saved", name: h.name }, h.name)}
          onedit={() => (editor = { host: h })}
          oncontext={(e) => (menu = { x: e.clientX, y: e.clientY, host: h })}
        />
      {:else}
        <li class="empty-state">
          <span class="ico"><Icon name="search" size={20} /></span>
          <h3>{app.hosts.length ? "Sin resultados" : "Carpeta vacía"}</h3>
          <p>{query ? `Ningún host coincide con «${query}».` : "No hay hosts en esta carpeta."}</p>
        </li>
      {/each}
    </ul>
  </section>
  {/if}

  {#if menu}
    {@const h = menu.host}
    <ContextMenu
      x={menu.x}
      y={menu.y}
      onclose={() => (menu = null)}
      items={[
        { label: "Conectar", run: () => openTab({ kind: "saved", name: h.name }, h.name) },
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
  ul { list-style: none; margin: 0 -10px; padding: 0; overflow: auto; display: grid; gap: 2px; align-content: start; }
  .first { grid-column: 2; }
</style>
