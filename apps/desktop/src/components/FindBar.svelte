<script lang="ts">
  import Icon from "./Icon.svelte";
  import { onMount } from "svelte";

  let { onfind, onclose }: { onfind: (q: string, dir: 1 | -1, incremental: boolean) => boolean; onclose: () => void } = $props();
  let q = $state("");
  let miss = $state(false);
  let input = $state<HTMLInputElement>();
  onMount(() => {
    input?.focus();
    input?.select();
  });

  function key(e: KeyboardEvent) {
    if (e.key === "Escape") {
      e.preventDefault();
      onclose();
    } else if (e.key === "Enter") {
      e.preventDefault();
      miss = q !== "" && !onfind(q, e.shiftKey ? -1 : 1, false);
    }
  }
</script>

<div class="find" role="search">
  <input
    bind:this={input}
    bind:value={q}
    placeholder="Buscar…"
    aria-label="Buscar en el terminal"
    class:miss
    oninput={() => (miss = q !== "" && !onfind(q, 1, true))}
    onkeydown={key}
  />
  <button class="icon-btn" aria-label="Anterior (Mayús+Intro)" title="Anterior (Mayús+Intro)" onclick={() => (miss = q !== "" && !onfind(q, -1, false))}><Icon name="chevronUp" /></button>
  <button class="icon-btn" aria-label="Siguiente (Intro)" title="Siguiente (Intro)" onclick={() => (miss = q !== "" && !onfind(q, 1, false))}><Icon name="chevronDown" /></button>
  <button class="icon-btn" aria-label="Cerrar búsqueda (Esc)" title="Cerrar (Esc)" onclick={onclose}><Icon name="x" /></button>
</div>

<style>
  .find { position: absolute; top: 8px; right: 18px; z-index: 5; display: flex; align-items: center; gap: 2px; padding: 4px; background: var(--bg-raised); border: 1px solid var(--border-strong); border-radius: var(--r); box-shadow: var(--shadow-2); }
  input { width: 210px; height: 28px; margin-right: 4px; }
  input.miss, input.miss:focus { border-color: var(--danger); box-shadow: 0 0 0 3px var(--danger-soft); }
</style>
