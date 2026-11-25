<script lang="ts">
  import { onMount } from "svelte";
  import { keyGenerate, keyImport, keyInstall, keysList } from "../lib/api";
  import { errorMessage } from "../lib/errors";
  import { sessions } from "../lib/sessions.svelte";
  import type { ConnId, KeyInfo } from "../lib/types";

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
  <header class="head">
    <h2>Llavero</h2>
    <div class="row">
      <button onclick={importKey}>Importar clave…</button>
      <button onclick={refresh} disabled={loading}>{loading ? "Actualizando…" : "Actualizar"}</button>
    </div>
  </header>

  {#if error}<p class="error">{error}</p>{/if}
  {#if notice}<p class="notice">{notice}</p>{/if}

  <div class="table-wrap">
    <table>
      <thead>
        <tr><th>Algoritmo</th><th>Comentario</th><th>Huella</th><th>Cifrada</th></tr>
      </thead>
      <tbody>
        {#each keys as k (k.path)}
          <tr class:picked={installKey === k.path} onclick={() => (installKey = k.path)}>
            <td>{k.algorithm}</td>
            <td>{k.comment || "—"}</td>
            <td class="mono">{k.fingerprint}</td>
            <td>{#if k.encrypted}<span class="badge">cifrada</span>{:else}<span class="dim">no</span>{/if}</td>
          </tr>
        {:else}
          <tr><td colspan="4" class="dim center">{loading ? "Cargando…" : "No hay claves públicas en ~/.ssh ni en el almacén"}</td></tr>
        {/each}
      </tbody>
    </table>
  </div>

  <div class="grid">
    <form class="card" onsubmit={(e) => { e.preventDefault(); void generate(); }}>
      <h3>Generar clave Ed25519</h3>
      <label>Nombre <input bind:value={name} placeholder="id_work" autocomplete="off" /></label>
      <label>Comentario <input bind:value={comment} placeholder="usuario@equipo" /></label>
      <label>Frase de paso (opcional) <input type="password" bind:value={passphrase} autocomplete="new-password" /></label>
      <label>Repetir frase <input type="password" bind:value={confirm} autocomplete="new-password" /></label>
      {#if genError}<p class="error">{genError}</p>{/if}
      <button type="submit" disabled={!name.trim() || !!genError || generating}>{generating ? "Generando…" : "Generar"}</button>
    </form>

    <div class="card">
      <h3>Instalar en un host</h3>
      <p class="dim">Selecciona una clave de la tabla y una conexión abierta.</p>
      <select bind:value={installConn}>
        <option value={null}>Conexión abierta…</option>
        {#each sessions.conns as c (c.id)}
          <option value={c.id}>{c.host}</option>
        {/each}
      </select>
      <p class="mono small">{installKey ?? "Ninguna clave seleccionada"}</p>
      <button onclick={install} disabled={installConn === null || !installKey}>Instalar clave</button>
    </div>
  </div>
</section>

<style>
  .view { display: flex; flex-direction: column; gap: 12px; }
  .head { display: flex; justify-content: space-between; align-items: center; }
  h2, h3 { margin: 0; }
  .row { display: flex; gap: 8px; }
  .table-wrap { border: 1px solid var(--border); border-radius: 8px; background: var(--bg-elev); overflow: auto; }
  table { width: 100%; border-collapse: collapse; font-size: 13px; }
  th, td { padding: 6px 10px; text-align: left; border-bottom: 1px solid var(--border); }
  th { color: var(--fg-dim); font-weight: 600; }
  tbody tr { cursor: pointer; }
  tbody tr:hover { background: var(--border); }
  tbody tr.picked { background: var(--term-selection); }
  .mono { font-family: var(--font-mono); font-size: 12px; word-break: break-all; }
  .small { font-size: 12px; }
  .dim { color: var(--fg-dim); }
  .center { text-align: center; }
  .badge { font-size: 11px; padding: 1px 6px; border-radius: 4px; border: 1px solid var(--accent); color: var(--accent); }
  .grid { display: grid; grid-template-columns: 1fr 1fr; gap: 12px; }
  .card { background: var(--bg-elev); border: 1px solid var(--border); border-radius: 8px; padding: 12px; display: flex; flex-direction: column; gap: 8px; }
  label { display: flex; flex-direction: column; gap: 4px; font-size: 13px; color: var(--fg-dim); }
  input, select { font: inherit; color: var(--fg); background: var(--bg); border: 1px solid var(--border); border-radius: 6px; padding: 5px 8px; }
  button { font: inherit; color: var(--fg); background: var(--bg-elev); border: 1px solid var(--border); border-radius: 6px; padding: 5px 12px; cursor: pointer; align-self: flex-start; }
  button:disabled { opacity: 0.5; cursor: default; }
  .error { color: var(--danger); margin: 0; }
  .notice { color: var(--accent); margin: 0; }
  @media (max-width: 720px) { .grid { grid-template-columns: 1fr; } }
</style>
