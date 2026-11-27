<script lang="ts">
  import Icon from "./Icon.svelte";

  let { value = $bindable(""), placeholder = "Buscar hosts…" }: { value?: string; placeholder?: string } = $props();
</script>

<label class="search">
  <span class="ico"><Icon name="search" size={15} /></span>
  <input type="search" bind:value {placeholder} aria-label="Buscar hosts" />
</label>

<style>
  .search { position: relative; flex: 1; min-width: 180px; max-width: 420px; display: flex; }
  .ico { position: absolute; left: 9px; top: 50%; transform: translateY(-50%); display: flex; color: var(--fg-faint); pointer-events: none; }
  input { width: 100%; padding-left: 31px; }
</style>
