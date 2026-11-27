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
  .menu { position: fixed; z-index: var(--z-menu); list-style: none; margin: 0; padding: 4px; min-width: 184px; background: var(--bg-raised); border: 1px solid var(--border-strong); border-radius: var(--r); box-shadow: var(--shadow-2); animation: menu-in 120ms var(--ease-out); }
  button { width: 100%; justify-content: flex-start; height: 28px; border: 0; background: none; padding: 0 10px; border-radius: 5px; font-weight: 400; }
  button:hover:not(:disabled), button:focus-visible { background: var(--accent-strong); color: var(--accent-fg); outline: none; }
  button.danger { color: var(--danger); background: none; }
  button.danger:hover:not(:disabled), button.danger:focus-visible { background: var(--danger-strong); color: #fff; }
  li:has(.danger) { border-top: 1px solid var(--border); margin-top: 4px; padding-top: 4px; }
  @keyframes menu-in { from { opacity: 0; transform: translateY(-3px); } }
</style>
