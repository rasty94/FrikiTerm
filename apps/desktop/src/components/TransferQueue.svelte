<script lang="ts">
  import { xferCancel, xferPause } from "../lib/api";
  import { errorMessage } from "../lib/errors";
  import { clearFinishedJobs, sessions, type Job } from "../lib/sessions.svelte";
  import { formatSize } from "../lib/sftp-logic";
  import Icon from "./Icon.svelte";

  const LABEL: Record<Job["status"], string> = {
    queued: "En cola", running: "Transfiriendo", retrying: "Reintentando", done: "Completada", failed: "Error", cancelled: "Cancelada",
  };
  const TONE: Record<Job["status"], string> = {
    queued: "", running: "accent", retrying: "warn", done: "ok", failed: "danger", cancelled: "",
  };

  let open = $state(true);
  let paused = $state(false);
  let error = $state<string | null>(null);

  const active = $derived(sessions.jobs.filter((j) => j.status === "queued" || j.status === "running" || j.status === "retrying"));
  const hasFinished = $derived(sessions.jobs.some((j) => !active.includes(j)));

  function pct(j: Job): number | null {
    if (!j.total || j.total <= 0) return null;
    return Math.min(100, Math.round((j.done / j.total) * 100));
  }

  async function cancel(j: Job) {
    error = null;
    try {
      await xferCancel(j.sftp, j.id);
    } catch (e) {
      error = errorMessage(e);
    }
  }

  async function togglePauseAll() {
    error = null;
    const next = !paused;
    const sftps = [...new Set(active.map((j) => j.sftp))];
    try {
      await Promise.all(sftps.map((s) => xferPause(s, next)));
      paused = next;
    } catch (e) {
      error = errorMessage(e);
    }
  }
</script>

<section class="queue panel" class:open aria-label="Transferencias">
  <header>
    <button class="toggle" onclick={() => (open = !open)} aria-expanded={open}>
      <span class="chev"><Icon name="chevronRight" size={14} /></span>
      Transferencias
      <span class="badge" class:accent={active.length > 0}>{active.length} activas</span>
    </button>
    <div class="actions">
      <button class="sm ghost" onclick={togglePauseAll} disabled={active.length === 0}><Icon name={paused ? "play" : "pause"} size={14} />{paused ? "Reanudar todo" : "Pausar todo"}</button>
      <button class="sm ghost" onclick={clearFinishedJobs} disabled={!hasFinished}>Limpiar</button>
    </div>
  </header>

  {#if open}
    {#if error}<p class="alert">{error}</p>{/if}
    <ul>
      {#each sessions.jobs as j (`${j.sftp}-${j.id}`)}
        <li>
          <div class="row">
            <span class="fico"><Icon name="file" size={15} /></span>
            <span class="file" title={j.file}>{j.file || `Trabajo ${j.id}`}</span>
            <span class="meta">
              {formatSize(j.done)}{j.total ? ` / ${formatSize(j.total)}` : ""}
              {#if j.error}<span class="err"> · {j.error}</span>{/if}
            </span>
            <span class="badge {TONE[j.status]}">{LABEL[j.status] ?? j.status}</span>
            {#if j.status === "queued" || j.status === "running" || j.status === "retrying"}
              <button class="icon-btn danger-hover" title="Cancelar" aria-label="Cancelar {j.file}" onclick={() => cancel(j)}><Icon name="x" size={14} /></button>
            {/if}
          </div>
          {#if j.status !== "done" && j.status !== "cancelled"}
            <div class="bar" class:indet={pct(j) === null} class:failed={j.status === "failed"}>
              <div style:width="{pct(j) ?? 100}%"></div>
            </div>
          {/if}
        </li>
      {:else}
        <li class="empty">Arrastra archivos entre paneles para transferirlos.</li>
      {/each}
    </ul>
  {/if}
</section>

<style>
  .queue { flex: none; }
  header { display: flex; justify-content: space-between; align-items: center; height: 40px; padding: 0 8px 0 6px; }
  .toggle { height: 30px; padding: 0 8px; gap: 8px; border: 0; background: none; font-weight: 600; }
  .chev { display: flex; color: var(--fg-dim); transition: transform 150ms var(--ease-out); }
  .open .chev { transform: rotate(90deg); }
  .actions { display: flex; gap: 2px; }
  ul { list-style: none; margin: 0; padding: 0 14px 8px; max-height: 200px; overflow: auto; border-top: 1px solid var(--border); }
  li { padding: 8px 0; border-bottom: 1px solid var(--border); }
  li:last-child { border-bottom: 0; }
  .row { display: flex; align-items: center; gap: 10px; font-size: 12.5px; }
  .fico { display: flex; color: var(--fg-faint); }
  .file { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; font-family: var(--font-mono); font-size: 12px; }
  .meta { font-size: 11.5px; color: var(--fg-dim); font-variant-numeric: tabular-nums; white-space: nowrap; }
  .err { color: var(--danger); }
  .bar { height: 3px; background: var(--bg-active); border-radius: 2px; overflow: hidden; margin: 7px 0 0 25px; }
  .bar > div { height: 100%; background: var(--accent); border-radius: 2px; transition: width 0.2s var(--ease-out); }
  .bar.failed > div { background: var(--danger); }
  .bar.indet > div { width: 30% !important; animation: slide 1.2s linear infinite; }
  @keyframes slide { from { transform: translateX(-100%); } to { transform: translateX(340%); } }
  .alert { margin: 0 14px 8px; }
  .empty { color: var(--fg-faint); font-size: 12px; padding: 10px 0 4px; border: 0; }
</style>
