<script lang="ts">
  import { onMount } from "svelte";
  import { hostDelete, hostsImportSshConfig, hostsList } from "../lib/api";
  import type { SessionConfig } from "../lib/types";
  import { app, openTab, toast } from "../lib/state.svelte";
  import { errMsg } from "../lib/session";
  import { buildFolderTree, hostsInFolder } from "../lib/folderTree";
  import { filterHosts } from "../lib/search";
  import FolderTree from "../components/FolderTree.svelte";
  import HostRow from "../components/HostRow.svelte";
  import HostForm from "../components/HostForm.svelte";
  import SearchBox from "../components/SearchBox.svelte";

  let folder = $state<string | null>(null);
  let query = $state("");
  let editor = $state<{ host: SessionConfig | null } | null>(null);

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
  }

  async function remove(h: SessionConfig) {
    if (!confirm(`¿Borrar el host «${h.name}»?`)) return;
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
        />
      {:else}
        <li class="empty">{app.hosts.length ? "Sin resultados" : "Aún no hay hosts. Crea uno o importa tu ssh_config."}</li>
      {/each}
    </ul>
  </section>

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
  .empty { padding: 24px; color: var(--fg-dim); }
</style>
