<script lang="ts">
  import { osInfo } from "../lib/hostMeta";
  import Icon from "./Icon.svelte";

  let { os, size = 15 }: { os?: string; size?: number } = $props();
  const info = $derived(osInfo(os));
</script>

<span class="os" class:unknown={!os} title={info.label} role="img" aria-label={info.label}>
  <Icon name={info.glyph} {size} />
  {#if info.badge}
    <span class="dot" style:--h={info.badge.hue} aria-hidden="true">{info.badge.letter}</span>
  {/if}
</span>

<style>
  .os { position: relative; display: inline-flex; color: var(--fg-dim); flex: none; }
  .os.unknown { color: var(--fg-faint); opacity: 0.6; }
  .dot {
    position: absolute; right: -5px; bottom: -4px; display: grid; place-items: center; width: 11px; height: 11px;
    border-radius: 50%; font-size: 7px; font-weight: 700; line-height: 1;
    background: oklch(var(--av-fg-l) var(--av-fg-c) var(--h)); color: var(--bg-elev);
  }
</style>
