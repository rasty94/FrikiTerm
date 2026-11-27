<script lang="ts">
  import type { HostMeta, SessionConfig } from "../lib/types";
  import { hueOf } from "../lib/folderTree";
  import { ago, initialsOf } from "../lib/hostMeta";
  import Icon from "./Icon.svelte";
  import OsBadge from "./OsBadge.svelte";

  let { host, title, meta, onconnect, onedit, oncontext }: { host: SessionConfig; title?: string; meta?: HostMeta; onconnect: () => void; onedit: () => void; oncontext: (e: MouseEvent) => void } = $props();

  const addr = $derived(`${host.user ? `${host.user}@` : ""}${host.host}:${host.port}`);
  // Color por carpeta de primer nivel (o por host si no tiene): los hosts de un grupo se reconocen de un vistazo.
  const hue = $derived(hueOf(host.folder.split("/")[0] || host.name));
</script>

<li class="row" oncontextmenu={(e) => { e.preventDefault(); oncontext(e); }}>
  <button class="open" data-nav data-name={host.name} onclick={onconnect} title={title ?? `Conectar a ${host.name}`}>
    <span class="avatar" style:--h={hue} aria-hidden="true">{initialsOf(host.name)}</span>
    <span class="text">
      <span class="name">{host.name}</span>
      <span class="addr">{addr}</span>
    </span>
    {#if host.folder}<span class="folder">{host.folder}</span>{/if}
    {#if meta?.lastConnected}<span class="last">{ago(meta.lastConnected)}</span>{/if}
    <OsBadge os={meta?.os} />
    <span class="badge proto">{host.protocol}</span>
  </button>
  <button class="icon-btn edit" aria-label="Editar {host.name}" title="Editar" onclick={onedit}>
    <Icon name="pencil" size={15} />
  </button>
</li>

<style>
  .row { position: relative; display: flex; align-items: center; border-radius: var(--r); }
  .row:hover { background: var(--bg-hover); }
  .open {
    flex: 1; min-width: 0; height: 52px; justify-content: flex-start; gap: 12px; padding: 0 44px 0 10px;
    border: 0; background: none; border-radius: var(--r); text-align: left; font-weight: 400;
  }
  .open:hover:not(:disabled), .open:active:not(:disabled) { background: none; }
  .open:focus-visible { outline-offset: -2px; }
  .avatar {
    display: grid; place-items: center; width: 34px; height: 34px; border-radius: 9px; flex: none;
    font-size: 12px; font-weight: 650; letter-spacing: 0.02em;
    background: oklch(var(--av-bg-l) var(--av-bg-c) var(--h));
    color: oklch(var(--av-fg-l) var(--av-fg-c) var(--h));
  }
  .text { display: grid; gap: 2px; min-width: 0; margin-right: auto; }
  .name { font-weight: 600; font-size: 13px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .addr { color: var(--fg-dim); font-family: var(--font-mono); font-size: 11.5px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .folder { color: var(--fg-faint); font-size: 12px; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; max-width: 30%; }
  .last { color: var(--fg-faint); font-size: 12px; white-space: nowrap; }
  .proto { text-transform: uppercase; letter-spacing: 0.03em; }
  .edit { position: absolute; right: 10px; opacity: 0; }
  .row:hover .edit, .edit:focus-visible { opacity: 1; }
</style>
