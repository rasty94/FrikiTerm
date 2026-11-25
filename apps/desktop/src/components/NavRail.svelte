<script lang="ts">
  import { app, type ViewName } from "../lib/state.svelte";

  const items: [ViewName, string][] = [
    ["hosts", "Hosts"],
    ["sftp", "SFTP"],
    ["keychain", "Llavero"],
    ["snippets", "Snippets"],
    ["forwards", "Redirecciones"],
  ];
</script>

<nav class="rail">
  {#each items as [name, label] (name)}
    <button class:active={app.view === name} onclick={() => (app.view = name)}>{label}</button>
  {/each}
</nav>

<style>
  .rail { display: flex; flex-direction: column; width: 96px; background: var(--bg-elev); border-right: 1px solid var(--border); }
  button { background: none; border: 0; color: var(--fg-dim); padding: 12px 8px; cursor: pointer; text-align: left; font: inherit; }
  button.active { color: var(--fg); border-left: 2px solid var(--accent); }
</style>
