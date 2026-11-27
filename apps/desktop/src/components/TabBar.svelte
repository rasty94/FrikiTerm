<script lang="ts">
  import { tick } from "svelte";
  import { app, closeOtherTabs, closePane, duplicateTab, moveTab, renameTab, requestCloseTab, splitPane, tabById, toggleBroadcast } from "../lib/state.svelte";
  import { leavesOf } from "../lib/layout";
  import Icon from "./Icon.svelte";
  import ContextMenu from "./ContextMenu.svelte";
  import NewTabMenu from "./NewTabMenu.svelte";

  let adding = $state<{ left: number; top: number } | null>(null);
  let plus = $state<HTMLButtonElement>();
  function openMenu() {
    const r = plus?.getBoundingClientRect();
    adding = { left: Math.max(8, Math.min((r?.left ?? 8), innerWidth - 338)), top: (r?.bottom ?? 40) + 4 };
  }
  let seenQuick = app.quick;
  $effect(() => {
    if (app.quick !== seenQuick) {
      seenQuick = app.quick;
      app.view = "terminal";
      tick().then(openMenu);
    }
  });
  const active = $derived(tabById(app.activeTab));
  const stateOf = (t: (typeof app.tabs)[number]) =>
    (leavesOf(t.root).find((l) => l.id === t.activeLeaf) ?? leavesOf(t.root)[0])?.status.state ?? "closed";

  // Renombrar (doble clic), reordenar (arrastrar) y menú contextual de pestaña.
  let renaming = $state<number | null>(null);
  let draft = $state("");
  let dragFrom = $state<number | null>(null);
  let over = $state<number | null>(null);
  let ctx = $state<{ x: number; y: number; id: number } | null>(null);
  function startRename(id: number) {
    renaming = id;
    draft = tabById(id)?.title ?? "";
    tick().then(() => document.querySelector<HTMLInputElement>(".rename")?.select());
  }
  function endRename(save: boolean) {
    if (renaming !== null && save) renameTab(renaming, draft);
    renaming = null;
  }
  function onDrop(i: number) {
    if (dragFrom !== null) moveTab(dragFrom, i);
    dragFrom = over = null;
  }

  // `id` es un valor, no estado: el menú cierra (ctx = null) antes de ejecutar la acción.
  function ctxItems(id: number) {
    const bc = tabById(id)?.broadcast;
    return [
      { label: "Renombrar", run: () => startRename(id) },
      { label: "Duplicar pestaña", run: () => duplicateTab(id) },
      { label: bc ? "Desactivar «Enviar a todos»" : "Activar «Enviar a todos»", run: () => toggleBroadcast(id) },
      { label: "Cerrar", run: () => void requestCloseTab(id), danger: true },
      ...(app.tabs.length > 1 ? [{ label: "Cerrar las demás", run: () => void closeOtherTabs(id), danger: true }] : []),
    ];
  }
</script>

<div class="bar">
  <div class="tabs" role="tablist">
    {#each app.tabs as t, i (t.id)}
      <!-- svelte-ignore a11y_no_static_element_interactions -->
      <div
        class="tab" class:on={t.id === app.activeTab} class:over={over === i && dragFrom !== i} class:dragging={dragFrom === i}
        draggable={renaming !== t.id}
        ondragstart={(e) => { dragFrom = i; e.dataTransfer?.setData("text/plain", t.title); }}
        ondragover={(e) => { if (dragFrom !== null) { e.preventDefault(); over = i; } }}
        ondrop={(e) => { e.preventDefault(); onDrop(i); }}
        ondragend={() => (dragFrom = over = null)}
        oncontextmenu={(e) => { e.preventDefault(); ctx = { x: e.clientX, y: e.clientY, id: t.id }; }}
      >
        <span class="dot {stateOf(t)}" aria-hidden="true"></span>
        {#if renaming === t.id}
          <input class="rename" bind:value={draft} maxlength="60" aria-label="Nombre de la pestaña"
            onkeydown={(e) => { if (e.key === "Enter") endRename(true); else if (e.key === "Escape") endRename(false); }} onblur={() => endRename(true)} />
        {:else}
          <button role="tab" class="label" aria-selected={t.id === app.activeTab} onclick={() => (app.activeTab = t.id)} ondblclick={() => startRename(t.id)} title="{t.title} (doble clic para renombrar)">{t.title}</button>
        {/if}
        {#if t.broadcast}<span class="bc" title="Enviar a todos activo" role="img" aria-label="Enviar a todos activo"><Icon name="broadcast" size={13} /></span>{/if}
        <button class="icon-btn close" aria-label="Cerrar {t.title}" title="Cerrar pestaña" onclick={() => requestCloseTab(t.id)}><Icon name="x" size={13} /></button>
      </div>
    {/each}
    <button class="icon-btn plus" data-newtab-btn bind:this={plus} aria-label="Nueva pestaña" title="Nueva pestaña" aria-expanded={adding !== null} aria-haspopup="dialog" onclick={() => (adding ? (adding = null) : openMenu())}><Icon name="plus" /></button>
  </div>
  {#if active}
    <div class="splits">
      <button class="icon-btn bcbtn" class:on={active.broadcast} aria-pressed={active.broadcast} onclick={() => toggleBroadcast(active.id)} title="Enviar a todos los paneles de la pestaña" aria-label="Enviar a todos"><Icon name="broadcast" /></button>
      <button class="icon-btn" onclick={() => splitPane(active.id, active.activeLeaf, "row")} title="Dividir a la derecha" aria-label="Dividir a la derecha"><Icon name="splitRight" /></button>
      <button class="icon-btn" onclick={() => splitPane(active.id, active.activeLeaf, "col")} title="Dividir abajo" aria-label="Dividir abajo"><Icon name="splitDown" /></button>
      <button class="icon-btn danger-hover" onclick={() => closePane(active.id, active.activeLeaf)} title="Cerrar panel activo" aria-label="Cerrar panel activo"><Icon name="x" /></button>
    </div>
  {/if}
</div>

{#if adding}<NewTabMenu left={adding.left} top={adding.top} onclose={() => (adding = null)} />{/if}
{#if ctx}
  <ContextMenu x={ctx.x} y={ctx.y} onclose={() => (ctx = null)} items={ctxItems(ctx.id)} />
{/if}

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
  .tab.dragging { opacity: 0.45; }
  .tab.over { box-shadow: inset 2px 0 0 var(--accent); }
  .rename { flex: 1; min-width: 0; height: 24px; padding: 0 6px; font-weight: 500; }
  .bc { display: flex; color: var(--accent); }
  .bcbtn.on { color: var(--accent-fg); background: var(--accent); }
  .splits { display: flex; align-items: center; gap: 2px; padding-left: 8px; border-left: 1px solid var(--border); margin: 8px 0; }
</style>
