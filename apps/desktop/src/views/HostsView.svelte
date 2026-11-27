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
  <aside class="folders">
    <button class="all" class:on={folder === null} aria-pressed={folder === null} onclick={() => (folder = null)}>
      Todos los hosts <span class="count">{app.hosts.length}</span>
    </button>
    <FolderTree nodes={tree.children} selected={folder} onselect={(p) => (folder = p)} />
  </aside>

  {#if loaded && app.hosts.length === 0}
    <section class="first">
      <h2>Bienvenido a FrikiTerm</h2>
      <p>Aún no tienes hosts guardados. Empieza importando los de tu <code>~/.ssh/config</code> o crea uno nuevo.</p>
      <div class="cta">
        <button class="primary" onclick={importSsh}>Importar ~/.ssh/config</button>
        <button onclick={() => (editor = { host: null })}>Nuevo host</button>
      </div>
    </section>
  {:else}
  <section class="list">
    <header>
      <SearchBox bind:value={query} />
      <button class="ghost" onclick={importSsh}>Importar ssh_config</button>
      <button class="primary" onclick={() => (editor = { host: null })}>+ Nuevo host</button>
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
        <li class="empty">{app.hosts.length ? "Sin resultados" : "Aún no hay hosts. Crea uno o importa tu ssh_config."}</li>
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
  .hosts { flex: 1; min-height: 0; display: grid; grid-template-columns: 200px minmax(0, 1fr); gap: 16px; }
  .folders { overflow: auto; border-right: 1px solid var(--border); padding-right: 8px; }
  .all { width: 100%; display: flex; justify-content: space-between; border: 0; background: none; border-radius: 6px; padding: 5px 8px; text-align: left; margin-bottom: 4px; }
  .all.on { background: var(--bg-hover); color: var(--accent); }
  .count { color: var(--fg-dim); font-size: 12px; }
  .list { display: flex; flex-direction: column; min-width: 0; min-height: 0; }
  header { display: flex; gap: 8px; align-items: center; margin-bottom: 12px; flex-wrap: wrap; }
  ul { list-style: none; margin: 0; padding: 0; overflow: auto; border-top: 1px solid var(--border); }
  .first { grid-column: 1 / -1; margin: auto; max-width: 440px; text-align: center; display: grid; gap: 12px; }
  .first h2 { margin: 0; }
  .first p { margin: 0; color: var(--fg-dim); }
  .cta { display: flex; gap: 8px; justify-content: center; }
  .empty { padding: 24px; color: var(--fg-dim); }
</style>
