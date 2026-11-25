<script lang="ts">
  import { xferCancel, xferPause } from "../lib/api";
  import { errorMessage } from "../lib/errors";
  import { clearFinishedJobs, sessions, type Job } from "../lib/sessions.svelte";
  import { formatSize } from "../lib/sftp-logic";

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

<section class="queue" class:open>
  <header>
    <button class="toggle" onclick={() => (open = !open)} aria-expanded={open}>
      Transferencias ({active.length} activas)
    </button>
    <div class="actions">
      <button onclick={togglePauseAll} disabled={active.length === 0}>{paused ? "Reanudar todo" : "Pausar todo"}</button>
      <button onclick={clearFinishedJobs} disabled={!hasFinished}>Limpiar</button>
    </div>
  </header>

  {#if open}
    {#if error}<p class="error">{error}</p>{/if}
    <ul>
      {#each sessions.jobs as j (`${j.sftp}-${j.id}`)}
        <li>
          <div class="row">
            <span class="file" title={j.file}>{j.file || `Trabajo ${j.id}`}</span>
            <span class="status {j.status}">{j.status}</span>
            {#if j.status === "queued" || j.status === "running" || j.status === "retrying"}
              <button class="cancel" onclick={() => cancel(j)}>Cancelar</button>
            {/if}
          </div>
          <div class="bar" class:indet={pct(j) === null}>
            <div style:width="{pct(j) ?? 100}%"></div>
          </div>
          <div class="meta">
            {formatSize(j.done)}{j.total ? ` / ${formatSize(j.total)}` : ""}
            {#if j.error}<span class="error"> · {j.error}</span>{/if}
          </div>
        </li>
      {:else}
        <li class="empty">Sin transferencias</li>
      {/each}
    </ul>
  {/if}
</section>

<style>
  .queue { border-top: 1px solid var(--border); background: var(--bg-elev); }
  header { display: flex; justify-content: space-between; align-items: center; padding: 6px 12px; }
  .toggle { background: none; border: 0; color: var(--fg); font: inherit; font-weight: 600; cursor: pointer; }
  .actions { display: flex; gap: 6px; }
  button { font: inherit; color: var(--fg); background: var(--bg); border: 1px solid var(--border); border-radius: 6px; padding: 3px 10px; cursor: pointer; }
  button:disabled { opacity: 0.5; cursor: default; }
  ul { list-style: none; margin: 0; padding: 0 12px 10px; max-height: 220px; overflow: auto; }
  li { padding: 6px 0; border-top: 1px solid var(--border); }
  .row { display: flex; align-items: center; gap: 8px; font-size: 13px; }
  .file { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; font-family: var(--font-mono); }
  .status { color: var(--fg-dim); font-size: 12px; }
  .status.failed { color: var(--danger); }
  .status.done { color: var(--accent); }
  .cancel { padding: 1px 8px; font-size: 12px; }
  .bar { height: 4px; background: var(--border); border-radius: 2px; overflow: hidden; margin: 4px 0; }
  .bar > div { height: 100%; background: var(--accent); transition: width 0.2s; }
  .bar.indet > div { width: 30% !important; animation: slide 1.2s linear infinite; }
  @keyframes slide { from { transform: translateX(-100%); } to { transform: translateX(340%); } }
  .meta { font-size: 12px; color: var(--fg-dim); }
  .error { color: var(--danger); margin: 4px 12px; }
  .empty { color: var(--fg-dim); font-size: 13px; }
</style>
