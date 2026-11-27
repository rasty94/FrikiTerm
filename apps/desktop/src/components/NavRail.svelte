<script lang="ts">
  import { app, type ViewName } from "../lib/state.svelte";
  import type { IconName } from "../lib/icons";
  import Icon from "./Icon.svelte";

  const items: { name: ViewName; label: string; icon: IconName }[] = [
    { name: "hosts", label: "Hosts", icon: "server" },
    { name: "terminal", label: "Terminal", icon: "terminal" },
    { name: "sftp", label: "SFTP", icon: "folder" },
    { name: "keychain", label: "Llavero", icon: "key" },
    { name: "snippets", label: "Snippets", icon: "braces" },
    { name: "forwards", label: "Redirecciones", icon: "arrows" },
  ];
</script>

{#snippet item(name: ViewName, label: string, icon: IconName)}
  <button class="item" class:active={app.view === name} aria-current={app.view === name ? "page" : undefined} onclick={() => (app.view = name)}>
    <span class="tile"><Icon name={icon} size={19} /></span>
    <span class="label">{label}</span>
  </button>
{/snippet}

<nav class="rail" aria-label="Principal">
  <div class="mark" aria-hidden="true"><Icon name="terminal" size={18} /></div>
  {#each items as it (it.name)}
    {@render item(it.name, it.label, it.icon)}
  {/each}
  <div class="bottom">{@render item("settings", "Ajustes", "sliders")}</div>
</nav>

<style>
  .rail { display: flex; flex-direction: column; align-items: stretch; gap: 2px; width: 88px; flex: none; padding: 12px 4px; box-sizing: border-box; background: var(--bg-sunken); border-right: 1px solid var(--border); }
  .mark { display: grid; place-items: center; width: 32px; height: 32px; margin: 0 auto 14px; border-radius: 9px; background: var(--accent-strong); color: var(--accent-fg); box-shadow: var(--shadow-1); }
  .item { flex-direction: column; gap: 3px; height: auto; padding: 6px 2px 7px; border: 0; background: none; color: var(--fg-dim); font-size: 10.5px; font-weight: 500; }
  .item:hover:not(:disabled) { background: none; color: var(--fg); }
  .item:active:not(:disabled) { background: none; }
  .tile { display: grid; place-items: center; width: 40px; height: 30px; border-radius: 8px; transition: background-color 120ms var(--ease-out), color 120ms var(--ease-out); }
  .item:hover .tile { background: var(--bg-hover); }
  .item.active { color: var(--fg); }
  .item.active .tile { background: var(--accent-soft); color: var(--accent); }
  .label { max-width: 100%; overflow: hidden; text-overflow: ellipsis; }
  .bottom { margin-top: auto; display: flex; flex-direction: column; }
</style>
