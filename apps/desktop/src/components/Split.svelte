<script lang="ts">
  import TerminalPane from "./TerminalPane.svelte";
  import { gridLayout, leavesOf } from "../lib/layout";
  import type { Tab } from "../lib/state.svelte";

  let { tab }: { tab: Tab } = $props();

  // Hojas planas en un único CSS grid: dividir no re-monta los terminales.
  const layout = $derived(gridLayout(tab.root));
  const leaves = $derived(leavesOf(tab.root));
</script>

<div class="stage" class:multi={leaves.length > 1} style="grid-template-columns:{layout.columns};grid-template-rows:{layout.rows}">
  {#each leaves as leaf (leaf.id)}
    <div class="cell" style="grid-column:{layout.slots[leaf.id].column};grid-row:{layout.slots[leaf.id].row}">
      <TerminalPane {leaf} tabId={tab.id} />
    </div>
  {/each}
</div>

<style>
  .stage { flex: 1; min-width: 0; min-height: 0; display: grid; gap: 1px; background: var(--border); }
  .cell { display: flex; min-width: 0; min-height: 0; }
</style>
