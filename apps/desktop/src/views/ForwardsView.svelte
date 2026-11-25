<script lang="ts">
  import { fwdList, fwdStart, fwdStop } from "../lib/api";
  import { errorMessage } from "../lib/errors";
  import { isLoopbackBind, validateForward, type FwdDraft } from "../lib/forward-logic";
  import { sessions } from "../lib/sessions.svelte";
  import type { ConnId, Forward, ForwardStatus } from "../lib/types";

  let conn = $state<ConnId | null>(null);
  let error = $state<string | null>(null);
  let busy = $state(false);

  let draft = $state<FwdDraft>({ kind: "local", bind: "", port: "", host: "", hostPort: "" });
  const check = $derived(validateForward(draft));

  const list = $derived<ForwardStatus[]>(conn === null ? [] : (sessions.forwards[conn] ?? []));

  // Elige la primera conexión abierta, y vuelve a ella si la elegida se cierra.
  $effect(() => {
    if (!sessions.conns.some((c) => c.id === conn)) conn = sessions.conns[0]?.id ?? null;
  });

  $effect(() => {
    const id = conn;
    if (id === null) return;
    fwdList(id)
      .then((l) => {
        if (conn === id) sessions.forwards[id] = l;
      })
      .catch((e) => (error = errorMessage(e)));
  });

  function describe(spec: Forward): string {
    if (spec.kind === "dynamic") return `SOCKS en ${spec.bind}:${spec.port}`;
    const arrow = spec.kind === "local" ? "→" : "←";
    return `${spec.bind}:${spec.port} ${arrow} ${spec.host}:${spec.host_port}`;
  }

  function kindLabel(k: Forward["kind"]): string {
    return k === "local" ? "-L" : k === "remote" ? "-R" : "-D";
  }

  function upsert(id: ConnId, st: ForwardStatus, replaceId?: number) {
    const cur = sessions.forwards[id] ?? [];
    const key = replaceId ?? st.id;
    const i = cur.findIndex((f) => f.id === key);
    if (i >= 0) cur[i] = st;
    else cur.push(st);
    sessions.forwards[id] = cur;
  }

  async function toggle(f: ForwardStatus) {
    if (conn === null || busy) return;
    const id = conn;
    busy = true;
    error = null;
    try {
      if (f.active) {
        await fwdStop(id, f.id);
        upsert(id, { ...f, active: false });
      } else {
        const st = await fwdStart(id, f.spec);
        upsert(id, st, f.id);
      }
    } catch (e) {
      error = errorMessage(e);
    } finally {
      busy = false;
    }
  }

  async function add() {
    if (conn === null || !check.ok) return;
    const id = conn;
    busy = true;
    error = null;
    try {
      const st = await fwdStart(id, check.spec);
      upsert(id, st);
      draft = { ...draft, port: "", host: "", hostPort: "" };
    } catch (e) {
      error = errorMessage(e);
    } finally {
      busy = false;
    }
  }
</script>

<section class="view">
  <header class="head">
    <h2>Redirecciones</h2>
    <select bind:value={conn}>
      <option value={null} disabled>Conexión abierta…</option>
      {#each sessions.conns as c (c.id)}
        <option value={c.id}>{c.host}</option>
      {/each}
    </select>
  </header>

  {#if error}<p class="error">{error}</p>{/if}

  {#if conn === null}
    <p class="dim">Abre una conexión SSH desde SFTP o Hosts para gestionar sus redirecciones.</p>
  {:else}
    <div class="table-wrap">
      <table>
        <thead><tr><th>Tipo</th><th>Redirección</th><th>Escucha</th><th>Estado</th><th></th></tr></thead>
        <tbody>
          {#each list as f (f.id)}
            <tr>
              <td class="mono">{kindLabel(f.spec.kind)}</td>
              <td class="mono">
                {describe(f.spec)}
                {#if !isLoopbackBind(f.spec.bind)}<span class="warn" title="Accesible desde otras máquinas">no loopback</span>{/if}
              </td>
              <td class="mono">{f.listen}</td>
              <td>
                {#if f.error}<span class="error">{f.error}</span>{:else}<span class={f.active ? "on" : "dim"}>{f.active ? "activa" : "parada"}</span>{/if}
              </td>
              <td class="right">
                <label class="switch" title={f.active ? "Parar" : "Arrancar"}>
                  <input type="checkbox" checked={f.active} onchange={() => toggle(f)} disabled={busy} />
                  <span></span>
                </label>
              </td>
            </tr>
          {:else}
            <tr><td colspan="5" class="dim center">Sin redirecciones en esta conexión</td></tr>
          {/each}
        </tbody>
      </table>
    </div>

    <form class="card" onsubmit={(e) => { e.preventDefault(); void add(); }}>
      <h3>Nueva redirección</h3>
      <div class="fields">
        <label>Tipo
          <select bind:value={draft.kind}>
            <option value="local">-L local</option>
            <option value="remote">-R remota</option>
            <option value="dynamic">-D SOCKS</option>
          </select>
        </label>
        <label>Escucha <input bind:value={draft.bind} placeholder="127.0.0.1" /></label>
        <label>Puerto <input bind:value={draft.port} inputmode="numeric" placeholder="8080" /></label>
        {#if draft.kind !== "dynamic"}
          <label>Host destino <input bind:value={draft.host} placeholder="db.internal" /></label>
          <label>Puerto destino <input bind:value={draft.hostPort} inputmode="numeric" placeholder="5432" /></label>
        {/if}
      </div>
      {#if !check.ok && (draft.port || draft.host || draft.hostPort)}<p class="error">{check.error}</p>{/if}
      {#if check.ok && check.warning}<p class="warn">Escucha en una interfaz no loopback: otras máquinas podrán conectar.</p>{/if}
      <button type="submit" disabled={!check.ok || busy}>Añadir</button>
    </form>
  {/if}
</section>

<style>
  .view { display: flex; flex-direction: column; gap: 12px; }
  .head { display: flex; justify-content: space-between; align-items: center; }
  h2, h3 { margin: 0; }
  .table-wrap { border: 1px solid var(--border); border-radius: 8px; background: var(--bg-elev); overflow: auto; }
  table { width: 100%; border-collapse: collapse; font-size: 13px; }
  th, td { padding: 6px 10px; text-align: left; border-bottom: 1px solid var(--border); white-space: nowrap; }
  th { color: var(--fg-dim); font-weight: 600; }
  td.right { text-align: right; }
  .mono { font-family: var(--font-mono); font-size: 12px; }
  .dim { color: var(--fg-dim); }
  .center { text-align: center; }
  .on { color: var(--accent); }
  .warn { color: var(--danger); font-size: 12px; margin-left: 6px; border: 1px solid var(--danger); border-radius: 4px; padding: 0 5px; }
  .error { color: var(--danger); margin: 0; }
  .card { background: var(--bg-elev); border: 1px solid var(--border); border-radius: 8px; padding: 12px; display: flex; flex-direction: column; gap: 10px; }
  .fields { display: grid; grid-template-columns: repeat(auto-fit, minmax(160px, 1fr)); gap: 10px; }
  label { display: flex; flex-direction: column; gap: 4px; font-size: 13px; color: var(--fg-dim); }
  input, select { font: inherit; color: var(--fg); background: var(--bg); border: 1px solid var(--border); border-radius: 6px; padding: 5px 8px; }
  button { font: inherit; color: var(--fg); background: var(--bg-elev); border: 1px solid var(--border); border-radius: 6px; padding: 5px 12px; cursor: pointer; align-self: flex-start; }
  button:disabled { opacity: 0.5; cursor: default; }
  .switch { position: relative; display: inline-block; width: 36px; height: 20px; }
  .switch input { opacity: 0; width: 0; height: 0; }
  .switch span { position: absolute; inset: 0; background: var(--border); border-radius: 20px; cursor: pointer; transition: background 0.2s; }
  .switch span::before { content: ""; position: absolute; width: 16px; height: 16px; left: 2px; top: 2px; background: var(--bg-elev); border-radius: 50%; transition: transform 0.2s; }
  .switch input:checked + span { background: var(--accent); }
  .switch input:checked + span::before { transform: translateX(16px); }
</style>
