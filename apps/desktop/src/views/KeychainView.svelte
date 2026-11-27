<script lang="ts">
  import { onMount } from "svelte";
  import { keyGenerate, keyImport, keyInstall, keysList } from "../lib/api";
  import { errorMessage } from "../lib/errors";
  import { sessions } from "../lib/sessions.svelte";
  import type { ConnId, KeyInfo } from "../lib/types";
  import Icon from "../components/Icon.svelte";

  let keys = $state<KeyInfo[]>([]);
  let loading = $state(false);
  let error = $state<string | null>(null);
  let notice = $state<string | null>(null);

  let name = $state("");
  let comment = $state("");
  let passphrase = $state("");
  let confirm = $state("");
  let generating = $state(false);

  let installConn = $state<ConnId | null>(null);
  let installKey = $state<string | null>(null);

  const genError = $derived(
    name.trim() === ""
      ? ""
      : /[\\/]/.test(name) || name.startsWith(".")
        ? "El nombre no puede contener / ni empezar por punto"
        : passphrase !== confirm
          ? "Las frases de paso no coinciden"
          : "",
  );

  async function refresh() {
    loading = true;
    error = null;
    try {
      keys = await keysList();
    } catch (e) {
      error = errorMessage(e);
    } finally {
      loading = false;
    }
  }

  async function generate() {
    if (!name.trim() || genError) return;
    generating = true;
    error = null;
    notice = null;
    try {
      const info = await keyGenerate(name.trim(), comment.trim(), passphrase === "" ? null : passphrase);
      notice = `Clave generada: ${info.fingerprint}`;
      name = "";
      comment = "";
      passphrase = "";
      confirm = "";
      await refresh();
    } catch (e) {
      error = errorMessage(e);
    } finally {
      generating = false;
    }
  }

  async function importKey() {
    error = null;
    notice = null;
    try {
      const info = await keyImport();
      if (info) {
        notice = `Importada: ${info.path}`;
        await refresh();
      }
    } catch (e) {
      error = errorMessage(e);
    }
  }

  async function install() {
    if (installConn === null || !installKey) return;
    error = null;
    notice = null;
    try {
      const added = await keyInstall(installConn, installKey);
      notice = added ? "Clave añadida a authorized_keys" : "La clave ya estaba presente en el host";
    } catch (e) {
      error = errorMessage(e);
    }
  }

  onMount(() => {
    void refresh();
  });
</script>

<section class="view">
  <header class="view-head">
    <div class="titles">
      <h1>Llavero</h1>
      <span class="sub">Claves públicas en ~/.ssh y en el almacén de FrikiTerm. Selecciona una para instalarla.</span>
    </div>
    <div class="actions">
      <button class="icon-btn" onclick={refresh} disabled={loading} title="Actualizar" aria-label="Actualizar"><Icon name="refresh" /></button>
      <button onclick={importKey}><Icon name="download" />Importar clave…</button>
    </div>
  </header>

  {#if error}<p class="alert" role="alert"><Icon name="alert" />{error}</p>{/if}
  {#if notice}<p class="alert ok" role="status"><Icon name="check" />{notice}</p>{/if}

  <div class="panel table-wrap">
    <table class="table">
      <thead>
        <tr><th class="ic"></th><th>Algoritmo</th><th>Comentario</th><th>Huella</th><th>Estado</th></tr>
      </thead>
      <tbody>
        {#each keys as k (k.path)}
          <tr class:picked={installKey === k.path} onclick={() => (installKey = k.path)} title={k.path}>
            <td class="ic"><span class="kico"><Icon name="key" size={15} /></span></td>
            <td class="algo">{k.algorithm}</td>
            <td>{k.comment || "—"}</td>
            <td class="mono fp">{k.fingerprint}</td>
            <td>{#if k.encrypted}<span class="badge accent"><Icon name="lock" size={11} />cifrada</span>{:else}<span class="badge">sin cifrar</span>{/if}</td>
          </tr>
        {:else}
          <tr class="none"><td colspan="5" class="center">{loading ? "Cargando…" : "No hay claves públicas en ~/.ssh ni en el almacén. Genera una abajo o importa una existente."}</td></tr>
        {/each}
      </tbody>
    </table>
  </div>

  <div class="grid">
    <form class="panel card" onsubmit={(e) => { e.preventDefault(); void generate(); }}>
      <h2>Generar clave Ed25519</h2>
      <div class="two">
        <label class="field">Nombre <input bind:value={name} placeholder="id_work" autocomplete="off" /></label>
        <label class="field">Comentario <input bind:value={comment} placeholder="usuario@equipo" /></label>
        <label class="field">Frase de paso (opcional) <input type="password" bind:value={passphrase} autocomplete="new-password" /></label>
        <label class="field">Repetir frase <input type="password" bind:value={confirm} autocomplete="new-password" /></label>
      </div>
      {#if genError}<p class="alert">{genError}</p>{/if}
      <div class="foot"><button type="submit" class="primary" disabled={!name.trim() || !!genError || generating}><Icon name="plus" />{generating ? "Generando…" : "Generar"}</button></div>
    </form>

    <div class="panel card">
      <h2>Instalar en un host</h2>
      <p class="sub">Añade la clave seleccionada a <code>authorized_keys</code> de una conexión abierta.</p>
      <label class="field">Conexión
        <select bind:value={installConn}>
          <option value={null}>{sessions.conns.length ? "Elegir conexión…" : "No hay conexiones abiertas"}</option>
          {#each sessions.conns as c (c.id)}
            <option value={c.id}>{c.host}</option>
          {/each}
        </select>
      </label>
      <div class="field">Clave
        <span class="picked-key" class:none={!installKey}>{installKey ?? "Selecciona una clave en la tabla"}</span>
      </div>
      <div class="foot"><button class="primary" onclick={install} disabled={installConn === null || !installKey}><Icon name="send" />Instalar clave</button></div>
    </div>
  </div>
</section>

<style>
  .table-wrap { overflow: auto; flex: none; }
  .ic { width: 20px; padding-right: 0; }
  .kico { display: flex; color: var(--fg-faint); }
  tbody tr { cursor: pointer; }
  tbody tr.picked, tbody tr.picked:hover { background: var(--accent-soft); }
  tbody tr.picked .kico { color: var(--accent); }
  tbody tr.none { cursor: default; }
  tbody tr.none:hover { background: none; }
  .algo { font-weight: 500; white-space: nowrap; }
  .fp { color: var(--fg-dim); word-break: break-all; }
  .center { text-align: center; color: var(--fg-faint); height: 64px; }
  .grid { display: grid; grid-template-columns: 1fr 1fr; gap: 16px; align-items: start; }
  .card { padding: 16px; display: flex; flex-direction: column; gap: 12px; }
  .card h2 { font-size: 13.5px; }
  .sub { margin: -6px 0 0; color: var(--fg-dim); font-size: 12.5px; }
  .two { display: grid; grid-template-columns: 1fr 1fr; gap: 12px; }
  .picked-key { font-family: var(--font-mono); font-size: 12px; color: var(--fg); padding: 7px 9px; border-radius: var(--r-sm); background: var(--bg-input); border: 1px dashed var(--border-strong); word-break: break-all; font-weight: 400; }
  .picked-key.none { color: var(--fg-faint); font-family: var(--font-ui); }
  .foot { display: flex; justify-content: flex-end; }
  @media (max-width: 860px) { .grid, .two { grid-template-columns: 1fr; } }
</style>
