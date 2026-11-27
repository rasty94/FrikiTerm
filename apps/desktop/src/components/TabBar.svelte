<script lang="ts">
  import { tick } from "svelte";
  import { app, closePane, openTab, requestCloseTab, splitPane, tabById, toast } from "../lib/state.svelte";
  import { defaultSession, parseQuickSpec } from "../lib/session";
  import { leavesOf } from "../lib/layout";
  import Icon from "./Icon.svelte";

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
  const stateOf = (t: (typeof app.tabs)[number]) =>
    (leavesOf(t.root).find((l) => l.id === t.activeLeaf) ?? leavesOf(t.root)[0])?.status.state ?? "closed";

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
        <span class="dot {stateOf(t)}" aria-hidden="true"></span>
        <button role="tab" class="label" aria-selected={t.id === app.activeTab} onclick={() => (app.activeTab = t.id)} title={t.title}>{t.title}</button>
        <button class="icon-btn close" aria-label="Cerrar {t.title}" title="Cerrar pestaña" onclick={() => requestCloseTab(t.id)}><Icon name="x" size={13} /></button>
      </div>
    {/each}
    <button class="icon-btn plus" aria-label="Conexión rápida" title="Conexión rápida" aria-expanded={adding} onclick={() => (adding = !adding)}><Icon name="plus" /></button>
    {#if adding}
      <form class="quick" onsubmit={quickConnect}>
        <span class="zap"><Icon name="zap" size={14} /></span>
        <input bind:value={spec} onkeydown={(e) => e.key === "Escape" && (adding = false)} placeholder="usuario@host:22" aria-label="Conexión rápida (usuario@host:puerto)" bind:this={input} />
        <button class="primary sm">Conectar</button>
      </form>
    {/if}
  </div>
  {#if active}
    <div class="splits">
      <button class="icon-btn" onclick={() => splitPane(active.id, active.activeLeaf, "row")} title="Dividir a la derecha" aria-label="Dividir a la derecha"><Icon name="splitRight" /></button>
      <button class="icon-btn" onclick={() => splitPane(active.id, active.activeLeaf, "col")} title="Dividir abajo" aria-label="Dividir abajo"><Icon name="splitDown" /></button>
      <button class="icon-btn danger-hover" onclick={() => closePane(active.id, active.activeLeaf)} title="Cerrar panel activo" aria-label="Cerrar panel activo"><Icon name="x" /></button>
    </div>
  {/if}
</div>

<style>
  .bar { display: flex; align-items: stretch; gap: 8px; height: 40px; padding: 0 8px 0 0; border-bottom: 1px solid var(--border); background: var(--bg-sunken); flex: none; }
  .tabs { display: flex; align-items: stretch; flex: 1; min-width: 0; overflow-x: auto; scrollbar-width: none; }
  .tab {
    position: relative; display: flex; align-items: center; gap: 8px; min-width: 120px; max-width: 220px; padding: 0 6px 0 14px;
    border-right: 1px solid var(--border); color: var(--fg-dim);
    transition: background-color 120ms var(--ease-out), color 120ms var(--ease-out);
  }
  .tab:hover { background: var(--bg-hover); color: var(--fg); }
  .tab.on { background: var(--term-bg); color: var(--fg); margin-bottom: -1px; }
  .tab.on::before { content: ""; position: absolute; left: 0; right: 0; top: 0; height: 2px; background: var(--accent); }
  .tab .label { flex: 1; min-width: 0; height: 100%; justify-content: flex-start; padding: 0; border: 0; background: none; color: inherit; font-weight: 500; overflow: hidden; text-overflow: ellipsis; }
  .tab .label:focus-visible { outline-offset: -2px; }
  .tab .close { width: 22px; height: 22px; opacity: 0; }
  .tab:hover .close, .tab.on .close, .tab .close:focus-visible { opacity: 1; }
  .plus { align-self: center; margin-left: 6px; }
  .quick { display: flex; align-items: center; gap: 6px; position: relative; margin-left: 6px; align-self: center; }
  .zap { position: absolute; left: 8px; display: flex; color: var(--accent); pointer-events: none; }
  .quick input { width: 230px; height: 28px; padding-left: 28px; font-family: var(--font-mono); font-size: 12px; }
  .splits { display: flex; align-items: center; gap: 2px; padding-left: 8px; border-left: 1px solid var(--border); margin: 8px 0; }
</style>
