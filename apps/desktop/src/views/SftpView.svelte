<script lang="ts">
  import { onMount } from "svelte";
  import { getCurrentWebview } from "@tauri-apps/api/webview";
  import FilePane, { type DragPayload } from "../components/FilePane.svelte";
  import TransferQueue from "../components/TransferQueue.svelte";
  import Icon from "../components/Icon.svelte";
  import {
    connClose,
    connOpen,
    fsList,
    fsMkdir,
    fsRemove,
    fsRename,
    localPickRoot,
    localRoot,
    sftpOpen,
    xferEnqueue,
    xferUploadPaths,
  } from "../lib/api";
  import { errorMessage } from "../lib/errors";
  import { addConn, queueJobs, removeConn, sessions, setConnSftp } from "../lib/sessions.svelte";
  import { app } from "../lib/state.svelte";
  import type { ConnId, EntryDto, FsRef } from "../lib/types";
  import { joinPath, rootLabel, type SortKey, type SortState } from "../lib/sftp-logic";

  type Side = "local" | "remote";
  interface Pane {
    fs: FsRef | null;
    path: string;
    entries: EntryDto[];
    loading: boolean;
    error: string | null;
    sort: SortState;
    selected: string | null;
  }
  type Op = { kind: "mkdir" } | { kind: "rename"; entry: EntryDto } | { kind: "delete"; entry: EntryDto };

  const newPane = (): Pane => ({
    fs: null,
    path: "/",
    entries: [],
    loading: false,
    error: null,
    sort: { key: "name", dir: "asc" },
    selected: null,
  });

  let local = $state<Pane>(newPane());
  let remote = $state<Pane>(newPane());
  let activeConn = $state<ConnId | null>(null);
  let hostName = $state("");
  let topError = $state<string | null>(null);
  let busy = $state(false);
  let op = $state<{ side: Side; op: Op } | null>(null);
  let opName = $state("");
  let remoteEl = $state<HTMLElement>();
  let overRemote = $state(false);

  const sshHosts = $derived(app.hosts.filter((h) => h.protocol === "ssh"));
  /** Nombre de la raíz local; su ruta (`local.path`) es relativa a ella ("" = la raíz). */
  const localName = $derived(local.fs?.kind === "local" ? rootLabel(local.fs.root) : "Local");
  const activeInfo = $derived(sessions.conns.find((c) => c.id === activeConn) ?? null);
  /** Valor del selector del panel remoto: "c:<id>" conexión abierta, "h:<nombre>" host guardado. */
  const remoteValue = $derived(activeConn !== null ? `c:${activeConn}` : "");

  async function pickRemote(el: HTMLSelectElement) {
    const v = el.value;
    if (v.startsWith("c:")) await selectConn(Number(v.slice(2)));
    else if (v.startsWith("h:")) {
      hostName = v.slice(2);
      await connectHost();
    }
    el.value = remoteValue; // refleja el estado real si la conexión falla
  }

  function paneOf(side: Side): Pane {
    return side === "local" ? local : remote;
  }

  function selectedEntry(p: Pane): EntryDto | null {
    return p.entries.find((e) => e.path === p.selected) ?? null;
  }

  async function load(p: Pane) {
    if (!p.fs) return;
    p.loading = true;
    p.error = null;
    try {
      const list = await fsList(p.fs, p.path);
      // LocalFs devuelve "./nombre"; el panel local trabaja con rutas relativas a su raíz.
      p.entries = p.fs.kind === "local" ? list.map((e) => ({ ...e, path: joinPath(p.path, e.name) })) : list;
    } catch (e) {
      p.entries = [];
      p.error = errorMessage(e);
    } finally {
      p.loading = false;
    }
  }

  function navigate(p: Pane, path: string) {
    p.path = path;
    p.selected = null;
    void load(p);
  }

  async function initLocal() {
    try {
      const root = await localRoot();
      local.fs = { kind: "local", root };
      local.path = "";
      await load(local);
    } catch (e) {
      local.error = errorMessage(e);
    }
  }

  async function pickLocalRoot() {
    try {
      const root = await localPickRoot();
      if (!root) return;
      local.fs = { kind: "local", root };
      local.path = "";
      local.selected = null;
      await load(local);
    } catch (e) {
      topError = errorMessage(e);
    }
  }

  async function connectHost() {
    if (!hostName) return;
    topError = null;
    busy = true;
    try {
      const id = await connOpen(hostName);
      addConn(id, hostName);
      await selectConn(id);
    } catch (e) {
      topError = errorMessage(e);
    } finally {
      busy = false;
    }
  }

  async function selectConn(id: ConnId) {
    topError = null;
    activeConn = id;
    const info = sessions.conns.find((c) => c.id === id);
    if (!info) return;
    try {
      if (info.sftp === null) setConnSftp(id, await sftpOpen(id));
      const sftp = sessions.conns.find((c) => c.id === id)?.sftp;
      if (sftp == null) return;
      remote.fs = { kind: "remote", sftp };
      remote.path = "/";
      remote.selected = null;
      await load(remote);
    } catch (e) {
      remote.fs = null;
      remote.entries = [];
      topError = errorMessage(e);
    }
  }

  async function disconnect() {
    if (activeConn === null) return;
    const id = activeConn;
    try {
      await connClose(id);
    } catch (e) {
      topError = errorMessage(e);
      return;
    }
    removeConn(id);
    activeConn = null;
    remote = newPane();
  }

  function fileName(path: string): string {
    return path.split(/[\\/]/).pop() ?? path;
  }

  async function enqueue(jobs: { direction: "upload" | "download"; src: string; dst: string; recursive: boolean; name: string }[]) {
    const sftp = activeInfo?.sftp;
    if (sftp == null || !local.fs || local.fs.kind !== "local") {
      topError = "Conecta un host y elige una carpeta local primero";
      return;
    }
    topError = null;
    try {
      const ids = await xferEnqueue(
        sftp,
        local.fs.root,
        jobs.map(({ name: _n, ...j }) => j),
      );
      queueJobs(sftp, ids, jobs.map((j) => j.name));
    } catch (e) {
      topError = errorMessage(e);
    }
  }

  /** Arrastrar una fila: de local a remoto es subida, de remoto a local es descarga. */
  function onRowTransfer(to: Side, src: DragPayload) {
    const target = paneOf(to);
    const dst = joinPath(target.path, src.name);
    if (to === "remote") {
      void enqueue([{ direction: "upload", src: src.path, dst, recursive: src.kind === "dir", name: src.name }]);
    } else {
      void enqueue([{ direction: "download", src: src.path, dst, recursive: src.kind === "dir", name: src.name }]);
    }
  }

  async function uploadOsPaths(paths: string[]) {
    const sftp = activeInfo?.sftp;
    if (sftp == null) {
      topError = "Conecta un host antes de soltar ficheros";
      return;
    }
    topError = null;
    try {
      const ids = await xferUploadPaths(sftp, paths, remote.path);
      queueJobs(sftp, ids, paths.map(fileName));
    } catch (e) {
      topError = errorMessage(e);
    }
  }

  function insideRemote(x: number, y: number): boolean {
    if (!remoteEl) return false;
    const r = remoteEl.getBoundingClientRect();
    const dpr = window.devicePixelRatio || 1;
    const px = x / dpr;
    const py = y / dpr;
    return px >= r.left && px <= r.right && py >= r.top && py <= r.bottom;
  }

  onMount(() => {
    void initLocal();
    // «Abrir SFTP» desde el menú de un host.
    if (app.sftpHost) {
      hostName = app.sftpHost;
      app.sftpHost = null;
      void connectHost();
    }
    let unlisten: (() => void) | undefined;
    // Fuera de Tauri (vite en navegador) no hay webview nativo: se ignora.
    getCurrentWebview()
      .onDragDropEvent((ev) => {
        const p = ev.payload;
        if (p.type === "enter" || p.type === "over") {
          overRemote = insideRemote(p.position.x, p.position.y);
        } else if (p.type === "drop") {
          const hit = insideRemote(p.position.x, p.position.y);
          overRemote = false;
          if (hit && p.paths.length > 0) void uploadOsPaths(p.paths);
        } else {
          overRemote = false;
        }
      })
      .then((u) => (unlisten = u))
      .catch(() => {});
    return () => unlisten?.();
  });

  function openOp(side: Side, o: Op) {
    opName = o.kind === "rename" ? o.entry.name : "";
    op = { side, op: o };
  }

  async function runOp() {
    if (!op) return;
    const { side, op: o } = op;
    const p = paneOf(side);
    if (!p.fs) return;
    busy = true;
    try {
      if (o.kind === "mkdir") {
        if (!opName.trim()) return;
        await fsMkdir(p.fs, joinPath(p.path, opName.trim()));
      } else if (o.kind === "rename") {
        if (!opName.trim()) return;
        await fsRename(p.fs, o.entry.path, joinPath(p.path, opName.trim()));
      } else {
        await fsRemove(p.fs, o.entry.path, o.entry.kind === "dir");
      }
      op = null;
      p.selected = null;
      await load(p);
    } catch (e) {
      topError = errorMessage(e);
    } finally {
      busy = false;
    }
  }

  function setSort(side: Side, key: SortKey) {
    const p = paneOf(side);
    p.sort = p.sort.key === key ? { key, dir: p.sort.dir === "asc" ? "desc" : "asc" } : { key, dir: "asc" };
  }

  const opTitle = $derived(
    op === null
      ? ""
      : op.op.kind === "mkdir"
        ? "Nueva carpeta"
        : op.op.kind === "rename"
          ? `Renombrar «${op.op.entry.name}»`
          : `Eliminar «${op.op.entry.name}»?`,
  );
</script>

{#snippet fileOps(side: Side)}
  {@const p = paneOf(side)}
  {@const sel = selectedEntry(p)}
  <button class="icon-btn" title="Nueva carpeta" aria-label="Nueva carpeta" onclick={() => openOp(side, { kind: "mkdir" })} disabled={!p.fs}><Icon name="folderPlus" /></button>
  <button class="icon-btn" title="Renombrar" aria-label="Renombrar" onclick={() => sel && openOp(side, { kind: "rename", entry: sel })} disabled={!sel}><Icon name="pencil" /></button>
  <button class="icon-btn danger-hover" title="Eliminar" aria-label="Eliminar" onclick={() => sel && openOp(side, { kind: "delete", entry: sel })} disabled={!sel}><Icon name="trash" /></button>
{/snippet}

{#snippet noConn()}
  <div class="empty-state">
    <span class="ico"><Icon name={busy ? "refresh" : "server"} size={20} /></span>
    <h3>{busy ? `Conectando a ${hostName}…` : "Sin conexión remota"}</h3>
    <p>
      {sshHosts.length || sessions.conns.length
        ? "Elige un host en la cabecera de este panel. Después arrastra archivos entre paneles para transferirlos."
        : "No hay hosts SSH guardados. Crea uno en Hosts para explorar sus archivos."}
    </p>
  </div>
{/snippet}

<section class="sftp">
  {#if topError}
    <p class="alert" role="alert"><Icon name="alert" />{topError}</p>
  {/if}

  <div class="panes">
    <FilePane
      side="local"
      title="Local"
      path={local.path}
      rootName={localName}
      entries={local.entries}
      loading={local.loading}
      error={local.error}
      sort={local.sort}
      selected={local.selected}
      onnavigate={(path) => navigate(local, path)}
      onsort={(key) => setSort("local", key)}
      onselect={(path) => (local.selected = path)}
      ontransfer={(src) => onRowTransfer("local", src)}
    >
      {#snippet headerExtra()}
        <button class="icon-btn" title="Elegir carpeta local…" aria-label="Elegir carpeta local" onclick={pickLocalRoot}><Icon name="folder" /></button>
        <span class="vsep" aria-hidden="true"></span>
        {@render fileOps("local")}
      {/snippet}
    </FilePane>

    <FilePane
      side="remote"
      title={activeInfo ? `Remoto · ${activeInfo.host}` : "Remoto"}
      path={remote.path}
      entries={remote.entries}
      loading={remote.loading}
      error={remote.error}
      sort={remote.sort}
      selected={remote.selected}
      dropHighlight={overRemote}
      placeholder={remote.fs ? undefined : noConn}
      bind:rootEl={remoteEl}
      onnavigate={(path) => navigate(remote, path)}
      onsort={(key) => setSort("remote", key)}
      onselect={(path) => (remote.selected = path)}
      ontransfer={(src) => onRowTransfer("remote", src)}
    >
      {#snippet heading()}
        <span class="host-pick">
          <span class="dot {activeInfo ? 'open' : busy ? 'connecting' : 'closed'}" aria-hidden="true"></span>
          <select aria-label="Host remoto" value={remoteValue} disabled={busy} onchange={(e) => void pickRemote(e.currentTarget)}>
            <option value="" disabled>{busy ? "Conectando…" : "Elegir host…"}</option>
            {#if sessions.conns.length}
              <optgroup label="Conexiones abiertas">
                {#each sessions.conns as c (c.id)}<option value="c:{c.id}">{c.host}</option>{/each}
              </optgroup>
            {/if}
            <optgroup label="Hosts guardados">
              {#each sshHosts as h (h.name)}<option value="h:{h.name}">{h.name}</option>{/each}
            </optgroup>
          </select>
        </span>
      {/snippet}
      {#snippet headerExtra()}
        {@render fileOps("remote")}
        <span class="vsep" aria-hidden="true"></span>
        <button class="icon-btn danger-hover" title="Desconectar" aria-label="Desconectar" onclick={disconnect} disabled={activeConn === null}><Icon name="power" /></button>
      {/snippet}
    </FilePane>
  </div>

  <TransferQueue />

  {#if op}
    <button class="scrim" aria-label="Cerrar" onclick={() => (op = null)}></button>
    <div class="dialog" role="dialog" aria-label={opTitle}>
      <form onsubmit={(e) => { e.preventDefault(); void runOp(); }}>
        <h3>{opTitle}</h3>
        {#if op.op.kind !== "delete"}
          <!-- svelte-ignore a11y_autofocus -->
          <input bind:value={opName} placeholder="Nombre" aria-label="Nombre" autofocus />
        {:else}
          <p>Esta acción no se puede deshacer.</p>
        {/if}
        <div class="actions">
          <button type="button" onclick={() => (op = null)}>Cancelar</button>
          <button type="submit" class={op.op.kind === "delete" ? "danger" : "primary"} disabled={busy}>
            {op.op.kind === "delete" ? "Eliminar" : "Aceptar"}
          </button>
        </div>
      </form>
    </div>
  {/if}
</section>

<style>
  .sftp { display: flex; flex-direction: column; gap: 12px; height: 100%; min-height: 0; }
  .panes { flex: 1; min-height: 0; display: grid; grid-template-columns: 1fr 1fr; gap: 12px; }
  .vsep { width: 1px; height: 16px; background: var(--border); margin: 0 4px; }
  .host-pick { position: relative; display: flex; align-items: center; flex: 1; min-width: 0; max-width: 260px; }
  .host-pick .dot { position: absolute; left: 10px; pointer-events: none; }
  .host-pick select { width: 100%; height: 30px; padding-left: 24px; font-weight: 600; background-color: var(--bg-input); }
  .scrim { position: fixed; inset: 0; height: auto; border: 0; border-radius: 0; background: var(--scrim); z-index: var(--z-modal); cursor: default; }
  .scrim:hover:not(:disabled) { background: var(--scrim); }
  .dialog { position: fixed; top: 28%; left: 50%; transform: translateX(-50%); z-index: calc(var(--z-modal) + 1); background: var(--bg-raised); border: 1px solid var(--border-strong); border-radius: 12px; padding: 20px; width: min(360px, calc(100vw - 32px)); box-sizing: border-box; box-shadow: var(--shadow-3); }
  .dialog form { display: flex; flex-direction: column; gap: 14px; }
  .dialog h3 { font-size: 15px; overflow-wrap: anywhere; }
  .dialog p { margin: 0; color: var(--fg-dim); }
  .dialog .actions { display: flex; justify-content: flex-end; gap: 8px; }
  @media (max-width: 720px) { .panes { grid-template-columns: 1fr; } }
</style>
