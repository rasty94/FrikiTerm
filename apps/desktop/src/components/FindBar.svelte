<script lang="ts">
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
  <button aria-label="Anterior (Mayús+Intro)" title="Anterior (Mayús+Intro)" onclick={() => (miss = q !== "" && !onfind(q, -1, false))}>↑</button>
  <button aria-label="Siguiente (Intro)" title="Siguiente (Intro)" onclick={() => (miss = q !== "" && !onfind(q, 1, false))}>↓</button>
  <button class="ghost" aria-label="Cerrar búsqueda (Esc)" onclick={onclose}>×</button>
</div>

<style>
  .find { position: absolute; top: 6px; right: 18px; z-index: 5; display: flex; gap: 4px; padding: 4px; background: var(--bg-elev); border: 1px solid var(--border); border-radius: 8px; box-shadow: 0 4px 16px rgb(0 0 0 / 0.25); }
  input { width: 200px; }
  input.miss { border-color: var(--danger); }
  button { padding: 4px 8px; }
</style>
