<script lang="ts">
  import { app, cycleTheme, type ViewName } from "../lib/state.svelte";

  const items: { name: ViewName; label: string; d: string }[] = [
    { name: "hosts", label: "Hosts", d: "M4 5h16v6H4zM4 13h16v6H4zM8 8h.01M8 16h.01" },
    { name: "terminal", label: "Terminal", d: "M4 6l5 6-5 6M12 18h8" },
    { name: "sftp", label: "SFTP", d: "M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z" },
    { name: "keychain", label: "Llavero", d: "M8 11V8a4 4 0 0 1 8 0v3M6 11h12v9H6z" },
    { name: "snippets", label: "Snippets", d: "M9 4H7v16h2M15 4h2v16h-2" },
    { name: "forwards", label: "Redirecciones", d: "M4 12h11M11 7l5 5-5 5M19 4v16" },
  ];
  const themeLabel = { system: "Auto", light: "Claro", dark: "Oscuro" };
</script>

<nav class="rail" aria-label="Principal">
  {#each items as it (it.name)}
    <button class="item" class:active={app.view === it.name} aria-current={app.view === it.name ? "page" : undefined} onclick={() => (app.view = it.name)}>
      <svg viewBox="0 0 24 24" aria-hidden="true"><path d={it.d} /></svg>
      <span>{it.label}</span>
    </button>
  {/each}
  <button class="item theme" onclick={cycleTheme} aria-label="Tema: {themeLabel[app.theme]}. Cambiar">
    <svg viewBox="0 0 24 24" aria-hidden="true"><circle cx="12" cy="12" r="9" /><path d="M12 3a9 9 0 0 1 0 18z" /></svg>
    <span>{themeLabel[app.theme]}</span>
  </button>
</nav>

<style>
  .rail { display: flex; flex-direction: column; gap: 2px; width: 84px; padding: 8px 6px; background: var(--bg-elev); border-right: 1px solid var(--border); }
  .item { display: flex; flex-direction: column; align-items: center; gap: 4px; background: none; border: 0; border-radius: 8px; padding: 10px 4px; color: var(--fg-dim); font-size: 11px; }
  .item:hover:not(:disabled) { background: var(--bg-hover); }
  .item.active { color: var(--accent); background: var(--bg-hover); }
  .item svg { width: 22px; height: 22px; fill: none; stroke: currentColor; stroke-width: 1.8; stroke-linecap: round; stroke-linejoin: round; }
  .item.theme { margin-top: auto; }
  .theme circle { fill: none; }
  .theme path { fill: currentColor; stroke: none; }
</style>
