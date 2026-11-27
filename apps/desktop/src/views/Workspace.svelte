<script lang="ts">
  import { app } from "../lib/state.svelte";
  import TabBar from "../components/TabBar.svelte";
  import Split from "../components/Split.svelte";
  import Icon from "../components/Icon.svelte";
</script>

<section class="ws">
  <TabBar />
  <div class="bodies">
    {#each app.tabs as tab (tab.id)}
      <div class="body" hidden={tab.id !== app.activeTab}><Split {tab} /></div>
    {:else}
      <div class="empty-state">
        <span class="ico"><Icon name="terminal" size={22} /></span>
        <h2>Sin sesiones abiertas</h2>
        <p>Abre un host guardado o conecta directamente con <code>usuario@host:puerto</code>.</p>
        <div class="cta">
          <button class="primary" onclick={() => (app.view = "hosts")}><Icon name="server" />Ir a Hosts</button>
          <button onclick={() => app.quick++}><Icon name="zap" />Conexión rápida</button>
        </div>
      </div>
    {/each}
  </div>
</section>

<style>
  .ws { flex: 1; min-height: 0; display: flex; flex-direction: column; }
  .bodies { flex: 1; min-height: 0; display: flex; background: var(--term-bg); }
  .body { flex: 1; min-width: 0; min-height: 0; display: flex; }
  .empty-state code { font-size: 12px; color: var(--fg); }
</style>
