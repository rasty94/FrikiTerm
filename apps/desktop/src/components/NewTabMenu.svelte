<script lang="ts">
  import { onMount } from "svelte";
  import { app, loadQuickHistory, openTab, rememberQuick, toast } from "../lib/state.svelte";
  import { hostsList } from "../lib/api";
  import { defaultSession, parseQuickSpec } from "../lib/session";
  import { filterHosts } from "../lib/search";
  import Icon from "./Icon.svelte";

  let { left, top, onclose }: { left: number; top: number; onclose: () => void } = $props();

  let root = $state<HTMLDivElement>();
  let quickInput = $state<HTMLInputElement>();
  let spec = $state("");
  let query = $state("");
  let history = $state(loadQuickHistory());
  const hosts = $derived(filterHosts(app.hosts, query).slice(0, 40));

  function connect(text: string) {
    const p = parseQuickSpec(text);
    if (!p) return toast("Formato: usuario@host:puerto", "info");
    openTab({ kind: "quick", spec: defaultSession({ host: p.host, port: p.port, user: p.user }) }, `${p.user ? `${p.user}@` : ""}${p.host}`);
    rememberQuick(text);
    onclose();
  }

  onMount(() => {
    quickInput?.focus();
    // Los hosts pueden no estar cargados si aún no se visitó la vista Hosts.
    hostsList().then((h) => (app.hosts = h)).catch(() => {});
    const away = (e: Event) => {
      const t = e.target as Element;
      if (!root?.contains(t) && !t.closest?.("[data-newtab-btn]")) onclose();
    };
    window.addEventListener("pointerdown", away, true);
    return () => window.removeEventListener("pointerdown", away, true);
  });
</script>

<div class="menu" role="dialog" aria-label="Nueva pestaña" bind:this={root} style="left:{left}px;top:{top}px" onkeydown={(e) => e.key === "Escape" && onclose()} tabindex="-1">
  <button class="item" onclick={() => { openTab({ kind: "local" }, "Local"); onclose(); }}>
    <Icon name="terminal" size={15} />Terminal local
  </button>

  <div class="sep"></div>
  <form class="quick" onsubmit={(e) => { e.preventDefault(); connect(spec); }}>
    <span class="zap"><Icon name="zap" size={14} /></span>
    <input bind:value={spec} bind:this={quickInput} placeholder="usuario@host:22" aria-label="Conexión rápida (usuario@host:puerto)" />
    <button class="primary sm">Conectar</button>
  </form>
  {#if history.length}
    <div class="hist" aria-label="Conexiones rápidas recientes">
      {#each history as h (h)}<button class="chip" onclick={() => connect(h)} title="Conectar a {h}">{h}</button>{/each}
    </div>
  {/if}

  <div class="sep"></div>
  <label class="find"><Icon name="search" size={14} /><input type="search" bind:value={query} placeholder="Buscar host guardado…" aria-label="Buscar host guardado" /></label>
  <ul class="hosts" aria-label="Hosts guardados">
    {#each hosts as h (h.name)}
      <li><button class="item" onclick={() => { openTab({ kind: "saved", name: h.name }, h.name); onclose(); }}>
        <Icon name="server" size={15} /><span class="nm">{h.name}</span><span class="sub">{h.user ? `${h.user}@` : ""}{h.host}</span>
      </button></li>
    {:else}
      <li class="none">{app.hosts.length ? "Ningún host coincide" : "Sin hosts guardados"}</li>
    {/each}
  </ul>
</div>

<style>
  .menu { position: fixed; z-index: var(--z-menu, 50); width: 330px; max-height: min(480px, calc(100vh - 56px)); display: flex; flex-direction: column; gap: 6px; padding: 8px; overflow: hidden; background: var(--bg-raised); border: 1px solid var(--border); border-radius: 10px; box-shadow: var(--shadow-2); }
  .sep { height: 1px; background: var(--border); margin: 2px -8px; }
  .item { display: flex; align-items: center; gap: 9px; width: 100%; height: 32px; padding: 0 8px; border: 0; border-radius: 6px; background: none; color: var(--fg); justify-content: flex-start; font-weight: 500; }
  .item:hover, .item:focus-visible { background: var(--bg-hover); }
  .nm { white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
  .sub { margin-left: auto; padding-left: 8px; color: var(--fg-faint); font: 11px var(--font-mono); white-space: nowrap; overflow: hidden; text-overflow: ellipsis; min-width: 0; }
  .quick { position: relative; display: flex; gap: 6px; align-items: center; }
  .zap { position: absolute; left: 8px; display: flex; color: var(--accent); pointer-events: none; }
  .quick input { flex: 1; min-width: 0; height: 30px; padding-left: 28px; font-family: var(--font-mono); font-size: 12px; }
  .hist { display: flex; flex-wrap: wrap; gap: 4px; max-height: 66px; overflow-y: auto; }
  .chip { height: 22px; padding: 0 8px; border-radius: 999px; font: 11px var(--font-mono); color: var(--fg-dim); background: var(--bg-sunken); border: 1px solid var(--border); }
  .chip:hover { color: var(--fg); background: var(--bg-hover); }
  .find { position: relative; display: flex; align-items: center; }
  .find :global(svg) { position: absolute; left: 9px; color: var(--fg-faint); pointer-events: none; }
  .find input { width: 100%; height: 30px; padding-left: 30px; }
  .hosts { list-style: none; margin: 0; padding: 0; overflow-y: auto; min-height: 0; }
  .none { padding: 10px 8px; color: var(--fg-faint); font-size: 12px; }
</style>
