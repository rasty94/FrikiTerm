<!-- Marcador: cola FIFO de prompts (app.prompts[0]). -->
<script lang="ts">
  import { app } from "../lib/state.svelte";
</script>

{#if app.prompts.length > 0}
  <div class="modal" role="dialog" aria-modal="true"></div>
{/if}
