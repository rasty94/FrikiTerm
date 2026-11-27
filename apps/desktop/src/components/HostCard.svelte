<script lang="ts">
  import type { HostMeta, SessionConfig } from "../lib/types";
  import { hueOf } from "../lib/folderTree";
  import { initialsOf } from "../lib/hostMeta";
  import OsBadge from "./OsBadge.svelte";

  let { host, title, meta, onconnect, oncontext }: { host: SessionConfig; title?: string; meta?: HostMeta; onconnect: () => void; oncontext: (e: MouseEvent) => void } = $props();

  const addr = $derived(`${host.user ? `${host.user}@` : ""}${host.host}${host.port === 22 ? "" : `:${host.port}`}`);
  const hue = $derived(hueOf(host.folder.split("/")[0] || host.name));
</script>

<li class="cell" oncontextmenu={(e) => { e.preventDefault(); oncontext(e); }}>
  <button class="card" data-nav data-name={host.name} onclick={onconnect} title={title ?? `Conectar a ${host.name}`}>
    <span class="top">
      <span class="avatar" style:--h={hue} aria-hidden="true">{initialsOf(host.name)}</span>
      <OsBadge os={meta?.os} size={16} />
    </span>
    <span class="name">{host.name}</span>
    <span class="addr">{addr}</span>
    <span class="foot">
      <span class="badge proto">{host.protocol}</span>
      {#if host.folder}<span class="folder">{host.folder}</span>{/if}
    </span>
  </button>
</li>

<style>
  .cell { list-style: none; min-width: 0; }
  .card {
    width: 100%; display: grid; gap: 4px; justify-content: stretch; justify-items: start; align-content: start; padding: 12px; height: 124px;
    text-align: left; font-weight: 400; background: var(--bg-elev); border: 1px solid var(--border);
    border-radius: var(--r-lg); box-shadow: var(--shadow-1);
  }
  .card:hover:not(:disabled) { background: var(--bg-hover); border-color: var(--border-strong); }
  .card:focus-visible { outline-offset: 2px; }
  .top { display: flex; align-items: center; justify-content: space-between; width: 100%; margin-bottom: 4px; }
  .avatar {
    display: grid; place-items: center; width: 34px; height: 34px; border-radius: 9px;
    font-size: 12px; font-weight: 650; letter-spacing: 0.02em;
    background: oklch(var(--av-bg-l) var(--av-bg-c) var(--h)); color: oklch(var(--av-fg-l) var(--av-fg-c) var(--h));
  }
  .name { font-weight: 600; font-size: 13px; max-width: 100%; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .addr { color: var(--fg-dim); font-family: var(--font-mono); font-size: 11.5px; max-width: 100%; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .foot { display: flex; gap: 8px; align-items: center; min-width: 0; max-width: 100%; margin-top: 2px; }
  .proto { text-transform: uppercase; letter-spacing: 0.03em; }
  .folder { color: var(--fg-faint); font-size: 12px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
</style>
