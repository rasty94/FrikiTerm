<script lang="ts">
  import { onMount } from "svelte";

  export interface MenuItem {
    label: string;
    run: () => void;
    danger?: boolean;
  }
  let { x, y, items, onclose }: { x: number; y: number; items: MenuItem[]; onclose: () => void } = $props();

  let el = $state<HTMLUListElement>();
  onMount(() => {
    el?.querySelector<HTMLButtonElement>("button")?.focus();
    const away = (e: Event) => {
      if (!el?.contains(e.target as Node)) onclose();
    };
    window.addEventListener("pointerdown", away, true);
    window.addEventListener("blur", onclose);
    return () => {
      window.removeEventListener("pointerdown", away, true);
      window.removeEventListener("blur", onclose);
    };
  });

  function key(e: KeyboardEvent) {
    const bs = [...(el?.querySelectorAll("button") ?? [])];
    const i = bs.indexOf(document.activeElement as HTMLButtonElement);
    if (e.key === "Escape") onclose();
    else if (e.key === "ArrowDown") bs[(i + 1) % bs.length]?.focus();
    else if (e.key === "ArrowUp") bs[(i - 1 + bs.length) % bs.length]?.focus();
    else return;
    e.preventDefault();
    e.stopPropagation();
  }

  // Dentro de la ventana aunque se abra junto al borde.
  const left = $derived(Math.max(4, Math.min(x, innerWidth - 190)));
  const top = $derived(Math.max(4, Math.min(y, innerHeight - items.length * 34 - 12)));
</script>

<ul class="menu" role="menu" bind:this={el} style="left:{left}px;top:{top}px" onkeydown={key}>
  {#each items as it (it.label)}
    <li role="none">
      <button role="menuitem" class:danger={it.danger} onclick={() => { onclose(); it.run(); }}>{it.label}</button>
    </li>
  {/each}
</ul>

<style>
  .menu { position: fixed; z-index: 70; list-style: none; margin: 0; padding: 4px; min-width: 180px; background: var(--bg-elev); border: 1px solid var(--border); border-radius: 8px; box-shadow: 0 8px 24px rgb(0 0 0 / 0.3); }
  button { display: block; width: 100%; text-align: left; border: 0; background: none; padding: 6px 10px; border-radius: 5px; }
  button.danger { color: var(--danger); }
</style>
