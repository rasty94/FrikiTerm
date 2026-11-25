<script lang="ts">
  import type { SessionConfig } from "../lib/types";

  let { host, onconnect, onedit }: { host: SessionConfig; onconnect: () => void; onedit: () => void } = $props();

  const addr = $derived(`${host.user ? `${host.user}@` : ""}${host.host}:${host.port}`);
</script>

<li class="row">
  <button class="open" onclick={onconnect} title="Conectar a {host.name}">
    <span class="proto">{host.protocol}</span>
    <span class="name">{host.name}</span>
    <span class="addr">{addr}</span>
  </button>
  <button class="ghost edit" aria-label="Editar {host.name}" onclick={onedit}>
    <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M4 20h4L19 9l-4-4L4 16z" /></svg>
  </button>
</li>

<style>
  .row { display: flex; align-items: center; gap: 6px; border-bottom: 1px solid var(--border); padding-right: 4px; }
  .open { flex: 1; min-width: 0; display: flex; align-items: center; gap: 10px; border: 0; background: none; border-radius: 0; text-align: left; padding: 10px 12px; }
  .proto { font-size: 10px; text-transform: uppercase; color: var(--fg-dim); border: 1px solid var(--border); border-radius: 4px; padding: 1px 4px; }
  .name { font-weight: 600; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .addr { margin-left: auto; color: var(--fg-dim); font-family: var(--font-mono); font-size: 12px; white-space: nowrap; }
  .edit { padding: 6px; }
  .edit svg { width: 16px; height: 16px; display: block; fill: none; stroke: currentColor; stroke-width: 2; stroke-linecap: round; stroke-linejoin: round; }
</style>
