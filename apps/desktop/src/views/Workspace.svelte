<script lang="ts">
  import { app, dismissRestore, restoreWorkspace, tabById, toggleBroadcast } from "../lib/state.svelte";
  import { isMac, matchShortcut } from "../lib/shortcuts";
  import TabBar from "../components/TabBar.svelte";
  import Split from "../components/Split.svelte";
  import Icon from "../components/Icon.svelte";

  const mac = isMac();
  // «Enviar a todos»: el resto de atajos vive en App.svelte; este solo existe con la vista Terminal montada.
  function onKeydown(e: KeyboardEvent) {
    if (app.confirm || app.prompts.length || matchShortcut(e, mac) !== "broadcast") return;
    const tab = tabById(app.activeTab);
    if (!tab) return;
    e.preventDefault();
    toggleBroadcast(tab.id);
  }
  const restoreCount = $derived(app.restore?.tabs.length ?? 0);
</script>

<svelte:window onkeydowncapture={onKeydown} />

<section class="ws">
  <TabBar />
  {#if app.restore}
    <div class="restore" role="region" aria-label="Restaurar sesiones anteriores">
      <Icon name="refresh" size={15} />
      <span>Tienes {restoreCount === 1 ? "1 pestaña" : `${restoreCount} pestañas`} de la sesión anterior. Se reconectará al restaurar.</span>
      <button class="primary sm" onclick={restoreWorkspace}>Restaurar sesiones anteriores</button>
      <button class="sm" onclick={dismissRestore}>Descartar</button>
    </div>
  {/if}
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
  .restore { display: flex; align-items: center; gap: 10px; flex: none; padding: 8px 14px; font-size: 12.5px; color: var(--fg); background: var(--accent-soft); border-bottom: 1px solid var(--accent-line); }
  .restore span { flex: 1; min-width: 0; }
  .bodies { flex: 1; min-height: 0; display: flex; background: var(--term-bg); }
  .body { flex: 1; min-width: 0; min-height: 0; display: flex; }
  .empty-state code { font-size: 12px; color: var(--fg); }
</style>
