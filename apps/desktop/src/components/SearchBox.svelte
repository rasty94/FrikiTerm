<script lang="ts">
  let { value = $bindable(""), placeholder = "Buscar hosts…" }: { value?: string; placeholder?: string } = $props();
</script>

<label class="search">
  <svg viewBox="0 0 24 24" aria-hidden="true"><circle cx="11" cy="11" r="6" /><path d="M20 20l-4.5-4.5" /></svg>
  <input type="search" bind:value {placeholder} aria-label="Buscar hosts" />
</label>

<style>
  .search { position: relative; flex: 1; min-width: 160px; }
  svg { position: absolute; left: 8px; top: 50%; transform: translateY(-50%); width: 16px; height: 16px; fill: none; stroke: var(--fg-dim); stroke-width: 2; pointer-events: none; }
  input { width: 100%; padding-left: 30px; box-sizing: border-box; }
</style>
