<script lang="ts">
  import { tick } from "svelte";
  import { app, closePane, openTab, requestCloseTab, splitPane, tabById, toast } from "../lib/state.svelte";
  import { defaultSession, parseQuickSpec } from "../lib/session";

  let adding = $state(false);
  let spec = $state("");
  let input = $state<HTMLInputElement>();
  $effect(() => {
    if (adding) tick().then(() => input?.focus());
  });
  let seenQuick = app.quick;
  $effect(() => {
    if (app.quick !== seenQuick) {
      seenQuick = app.quick;
      adding = true;
      app.view = "terminal";
    }
  });
  const active = $derived(tabById(app.activeTab));

  function quickConnect(e: SubmitEvent) {
    e.preventDefault();
    const p = parseQuickSpec(spec);
    if (!p) return toast("Formato: usuario@host:puerto", "info");
    const session = defaultSession({ host: p.host, port: p.port, user: p.user });
    openTab({ kind: "quick", spec: session }, `${p.user ? `${p.user}@` : ""}${p.host}`);
    spec = "";
    adding = false;
  }
</script>

<div class="bar">
  <div class="tabs" role="tablist">
    {#each app.tabs as t (t.id)}
      <div class="tab" class:on={t.id === app.activeTab}>
        <button role="tab" class="label" aria-selected={t.id === app.activeTab} onclick={() => (app.activeTab = t.id)}>{t.title}</button>
        <button class="close" aria-label="Cerrar {t.title}" onclick={() => requestCloseTab(t.id)}>×</button>
      </div>
    {/each}
    <button class="plus" aria-label="Conexión rápida" aria-expanded={adding} onclick={() => (adding = !adding)}>+</button>
  </div>
  {#if adding}
    <form class="quick" onsubmit={quickConnect}>
      <input bind:value={spec} onkeydown={(e) => e.key === "Escape" && (adding = false)} placeholder="usuario@host:22" aria-label="Conexión rápida (usuario@host:puerto)" bind:this={input} />
      <button class="primary">Conectar</button>
    </form>
  {/if}
  {#if active}
    <div class="splits">
      <button onclick={() => splitPane(active.id, active.activeLeaf, "row")} title="Dividir a la derecha">Dividir ↔</button>
      <button onclick={() => splitPane(active.id, active.activeLeaf, "col")} title="Dividir abajo">Dividir ↕</button>
      <button class="ghost" onclick={() => closePane(active.id, active.activeLeaf)} title="Cerrar panel activo">Cerrar panel</button>
    </div>
  {/if}
</div>

<style>
  .bar { display: flex; align-items: center; gap: 8px; padding: 6px; border-bottom: 1px solid var(--border); background: var(--bg-elev); flex-wrap: wrap; }
  .tabs { display: flex; gap: 4px; flex: 1; min-width: 0; overflow-x: auto; }
  .tab { display: flex; align-items: center; border: 1px solid var(--border); border-radius: 6px; background: var(--bg); }
  .tab.on { border-color: var(--accent); }
  .tab .label { border: 0; background: none; padding: 4px 8px; }
  .tab .close { border: 0; background: none; padding: 4px 6px; color: var(--fg-dim); }
  .plus { padding: 4px 10px; }
  .quick { display: flex; gap: 6px; }
  .quick input { width: 240px; }
  .splits { display: flex; gap: 4px; }
</style>
