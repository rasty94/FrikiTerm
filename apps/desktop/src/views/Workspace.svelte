<script lang="ts">
  import { app } from "../lib/state.svelte";
  import TabBar from "../components/TabBar.svelte";
  import Split from "../components/Split.svelte";
</script>

<section class="ws">
  <TabBar />
  <div class="bodies">
    {#each app.tabs as tab (tab.id)}
      <div class="body" hidden={tab.id !== app.activeTab}><Split {tab} /></div>
    {:else}
      <p class="empty">Sin sesiones. Abre un host desde <em>Hosts</em> o usa <strong>+</strong> para una conexión rápida.</p>
    {/each}
  </div>
</section>

<style>
  .ws { flex: 1; min-height: 0; display: flex; flex-direction: column; }
  .bodies { flex: 1; min-height: 0; display: flex; padding: 6px; }
  .body { flex: 1; min-width: 0; min-height: 0; display: flex; }
  .empty { margin: auto; color: var(--fg-dim); }
</style>
