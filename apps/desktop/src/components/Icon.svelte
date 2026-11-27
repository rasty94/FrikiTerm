<script lang="ts">
  import { ICONS, type IconName } from "../lib/icons";

  let { name, size = 16 }: { name: IconName; size?: number } = $props();
</script>

<svg class="icon" width={size} height={size} viewBox="0 0 24 24" aria-hidden="true">{@html ICONS[name]}</svg>

<style>
  svg { flex: none; fill: none; stroke: currentColor; stroke-width: 1.75; stroke-linecap: round; stroke-linejoin: round; }
</style>
