<script lang="ts">
  import { onMount } from "svelte";
  import Icon from "./Icon.svelte";
  import type { IconName } from "../lib/icons";

  export interface MenuItem {
    label: string;
    run: () => void;
    danger?: boolean;
    /** Si falta, se usa el icono de la etiqueta (ver DEFAULT_ICONS). */
    icon?: IconName;
  }
  let { x, y, items, onclose }: { x: number; y: number; items: MenuItem[]; onclose: () => void } = $props();

  // ponytail: iconos por etiqueta para los menús existentes; los nuevos pueden pasar `icon`.
  const DEFAULT_ICONS: Record<string, IconName> = {
    Conectar: "terminal",
    "Abrir SFTP": "folder",
    Duplicar: "layers",
    Editar: "pencil",
    Eliminar: "trash",
    Copiar: "copy",
    Pegar: "clipboard",
    "Buscar…": "search",
    "Limpiar terminal": "eraser",
  };

  let el = $state<HTMLUListElement>();
  // Tamaño medido tras montar: el menú se mantiene dentro de la ventana aunque se abra junto al borde.
  let size = $state({ w: 0, h: 0 });

  const left = $derived(Math.max(4, Math.min(x, innerWidth - size.w - 4)));
  const top = $derived(Math.max(4, Math.min(y, innerHeight - size.h - 4)));

  function menuItems() {
    return [...(el?.querySelectorAll<HTMLButtonElement>("button[role=menuitem]") ?? [])];
  }

  onMount(() => {
    if (el) size = { w: el.offsetWidth, h: el.offsetHeight };
    menuItems()[0]?.focus();
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
    const bs = menuItems();
    const i = bs.indexOf(document.activeElement as HTMLButtonElement);
    const n = bs.length;
    if (e.key === "Escape" || e.key === "Tab") onclose();
    else if (!n) return;
    else if (e.key === "ArrowDown") bs[(i + 1) % n]?.focus();
    else if (e.key === "ArrowUp") bs[(i - 1 + n) % n]?.focus();
    else if (e.key === "Home") bs[0]?.focus();
    else if (e.key === "End") bs[n - 1]?.focus();
    else return;
    // Tab sigue su curso normal (cierra el menú); el resto de teclas no llegan a la ventana.
    if (e.key !== "Tab") {
      e.preventDefault();
      e.stopPropagation();
    }
  }
</script>

<ul
  class="menu"
  role="menu"
  aria-label="Acciones"
  bind:this={el}
  style="left:{left}px;top:{top}px"
  onkeydown={key}
>
  {#each items as it (it.label)}
    {@const icon = it.icon ?? DEFAULT_ICONS[it.label]}
    <li role="none">
      <button role="menuitem" class:danger={it.danger} onclick={() => { onclose(); it.run(); }}>
        {#if icon}<span class="ico" aria-hidden="true"><Icon name={icon} size={15} /></span>{/if}
        <span>{it.label}</span>
      </button>
    </li>
  {/each}
</ul>

<style>
  .menu {
    position: fixed; z-index: var(--z-menu); list-style: none; margin: 0; padding: 4px; min-width: 188px;
    background: var(--bg-raised); border: 1px solid var(--border-strong); border-radius: var(--r); box-shadow: var(--shadow-2);
    animation: menu-in 120ms var(--ease-out);
  }
  li { margin: 0; padding: 0; }
  button {
    display: flex; align-items: center; gap: 9px; width: 100%; justify-content: flex-start; height: 32px;
    padding: 0 10px 0 8px; border: 0; border-radius: 6px; background: none; color: var(--fg); font-weight: 400; text-align: left;
  }
  .ico { display: grid; place-items: center; color: var(--fg-dim); flex: none; }
  button:hover:not(:disabled), button:focus-visible { background: var(--accent-strong); color: var(--accent-fg); outline: none; }
  button:hover:not(:disabled) .ico, button:focus-visible .ico { color: inherit; }
  button.danger { color: var(--danger); }
  button.danger .ico { color: var(--danger); }
  button.danger:hover:not(:disabled), button.danger:focus-visible { background: var(--danger); color: #fff; }
  button.danger:hover:not(:disabled) .ico, button.danger:focus-visible .ico { color: #fff; }
  @keyframes menu-in { from { opacity: 0; transform: scale(0.98); } }
</style>
