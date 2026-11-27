<script lang="ts">
  import { fwdList, fwdStart, fwdStop } from "../lib/api";
  import { errorMessage } from "../lib/errors";
  import { isLoopbackBind, validateForward, type FwdDraft } from "../lib/forward-logic";
  import { sessions } from "../lib/sessions.svelte";
  import type { ConnId, Forward, ForwardStatus } from "../lib/types";
  import Icon from "../components/Icon.svelte";
  import { app } from "../lib/state.svelte";

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
  <header class="view-head">
    <div class="titles">
      <h1>Redirecciones</h1>
      <span class="sub">Túneles de puertos (-L, -R, -D) sobre una conexión SSH abierta.</span>
    </div>
    <div class="actions">
      <span class="conn">
        <span class="dot {conn === null ? 'closed' : 'open'}" aria-hidden="true"></span>
        <select bind:value={conn} aria-label="Conexión">
          <option value={null} disabled>{sessions.conns.length ? "Conexión abierta…" : "Sin conexiones"}</option>
          {#each sessions.conns as c (c.id)}
            <option value={c.id}>{c.host}</option>
          {/each}
        </select>
      </span>
    </div>
  </header>

  {#if error}<p class="alert" role="alert"><Icon name="alert" />{error}</p>{/if}

  {#if conn === null}
    <div class="empty-state">
      <span class="ico"><Icon name="arrows" size={20} /></span>
      <h3>Sin conexión SSH abierta</h3>
      <p>Abre un host desde Hosts o SFTP; después podrás crear y activar sus redirecciones aquí.</p>
      <div class="cta"><button class="primary" onclick={() => (app.view = "hosts")}><Icon name="server" />Ir a Hosts</button></div>
    </div>
  {:else}
    <div class="panel table-wrap">
      <table class="table">
        <thead><tr><th>Tipo</th><th>Redirección</th><th>Escucha</th><th>Estado</th><th class="right">Activa</th></tr></thead>
        <tbody>
          {#each list as f (f.id)}
            <tr>
              <td><span class="badge kind">{kindLabel(f.spec.kind)}</span></td>
              <td class="mono">
                {describe(f.spec)}
                {#if !isLoopbackBind(f.spec.bind)}<span class="badge warn" title="Accesible desde otras máquinas">no loopback</span>{/if}
              </td>
              <td class="mono dim">{f.listen}</td>
              <td>
                {#if f.error}<span class="state err"><span class="dot failed"></span>{f.error}</span>
                {:else}<span class="state"><span class="dot {f.active ? 'open' : 'closed'}"></span>{f.active ? "Activa" : "Parada"}</span>{/if}
              </td>
              <td class="right">
                <label class="switch" title={f.active ? "Parar" : "Arrancar"}>
                  <input type="checkbox" role="switch" checked={f.active} onchange={() => toggle(f)} disabled={busy} aria-label="{f.active ? 'Parar' : 'Arrancar'} {describe(f.spec)}" />
                  <span></span>
                </label>
              </td>
            </tr>
          {:else}
            <tr class="none"><td colspan="5" class="center">Esta conexión aún no tiene redirecciones. Añade una abajo.</td></tr>
          {/each}
        </tbody>
      </table>
    </div>

    <form class="panel card" onsubmit={(e) => { e.preventDefault(); void add(); }}>
      <h2>Nueva redirección</h2>
      <div class="fields">
        <label class="field">Tipo
          <select bind:value={draft.kind}>
            <option value="local">-L local</option>
            <option value="remote">-R remota</option>
            <option value="dynamic">-D SOCKS</option>
          </select>
        </label>
        <label class="field">Escucha <input bind:value={draft.bind} placeholder="127.0.0.1" /></label>
        <label class="field">Puerto <input bind:value={draft.port} inputmode="numeric" placeholder="8080" /></label>
        {#if draft.kind !== "dynamic"}
          <label class="field">Host destino <input bind:value={draft.host} placeholder="db.internal" /></label>
          <label class="field">Puerto destino <input bind:value={draft.hostPort} inputmode="numeric" placeholder="5432" /></label>
        {/if}
      </div>
      {#if !check.ok && (draft.port || draft.host || draft.hostPort)}<p class="alert">{check.error}</p>{/if}
      {#if check.ok && check.warning}<p class="alert warn">Escucha en una interfaz no loopback: otras máquinas podrán conectar.</p>{/if}
      <div class="foot"><button type="submit" class="primary" disabled={!check.ok || busy}><Icon name="plus" />Añadir</button></div>
    </form>
  {/if}
</section>

<style>
  .conn { position: relative; display: flex; align-items: center; }
  .conn .dot { position: absolute; left: 10px; pointer-events: none; }
  .conn select { padding-left: 24px; min-width: 200px; font-weight: 500; }
  .table-wrap { overflow: auto; flex: none; }
  .table td { white-space: nowrap; }
  .kind { font-family: var(--font-mono); text-transform: none; color: var(--accent); background: var(--accent-soft); }
  .badge.warn { margin-left: 8px; }
  .dim { color: var(--fg-dim); }
  .right { text-align: right; }
  .center { text-align: center; color: var(--fg-faint); height: 64px; }
  tbody tr.none:hover { background: none; }
  .state { display: inline-flex; align-items: center; gap: 7px; }
  .state.err { color: var(--danger); white-space: normal; }
  .card { padding: 16px; display: flex; flex-direction: column; gap: 12px; }
  .card h2 { font-size: 13.5px; }
  .fields { display: grid; grid-template-columns: repeat(auto-fit, minmax(150px, 1fr)); gap: 12px; }
  .foot { display: flex; justify-content: flex-end; }
  .switch { position: relative; display: inline-block; width: 34px; height: 20px; vertical-align: middle; }
  .switch input { position: absolute; opacity: 0; width: 100%; height: 100%; margin: 0; cursor: pointer; z-index: 1; }
  .switch span { position: absolute; inset: 0; background: var(--border-strong); border-radius: 20px; transition: background-color 0.2s var(--ease-out); }
  .switch span::before { content: ""; position: absolute; width: 16px; height: 16px; left: 2px; top: 2px; background: #fff; border-radius: 50%; box-shadow: var(--shadow-1); transition: transform 0.2s var(--ease-out); }
  .switch input:checked + span { background: var(--accent-strong); }
  .switch input:checked + span::before { transform: translateX(14px); }
  .switch input:focus-visible + span { outline: 2px solid var(--accent); outline-offset: 2px; }
  .switch input:disabled + span { opacity: 0.5; }
</style>
