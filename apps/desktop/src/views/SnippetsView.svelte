<script lang="ts">
  import { onMount } from "svelte";
  import SnippetRunner from "../components/SnippetRunner.svelte";
  import { snippetsList, snippetsSave } from "../lib/api";
  import { errorMessage } from "../lib/errors";
  import { validateSnippets } from "../lib/snippet-logic";
  import type { Snippet } from "../lib/types";
  import Icon from "../components/Icon.svelte";

  let list = $state<Snippet[]>([]);
  let sel = $state<number | null>(null);
  let error = $state<string | null>(null);
  let notice = $state<string | null>(null);
  let loading = $state(false);

  const current = $derived(sel === null ? null : (list[sel] ?? null));

  async function load() {
    loading = true;
    error = null;
    try {
      list = await snippetsList();
      sel = list.length > 0 ? 0 : null;
    } catch (e) {
      error = errorMessage(e);
    } finally {
      loading = false;
    }
  }

  function add() {
    list.push({ name: `snippet-${list.length + 1}`, command: "" });
    sel = list.length - 1;
    notice = null;
  }

  function remove() {
    if (sel === null) return;
    list.splice(sel, 1);
    sel = list.length > 0 ? Math.min(sel, list.length - 1) : null;
    notice = null;
  }

  async function save() {
    const problem = validateSnippets(list);
    if (problem) {
      error = problem;
      return;
    }
    error = null;
    try {
      await snippetsSave($state.snapshot(list) as Snippet[]);
      notice = "Snippets guardados";
    } catch (e) {
      error = errorMessage(e);
    }
  }

  onMount(() => {
    void load();
  });
</script>

<section class="view">
  <header class="view-head">
    <div class="titles">
      <h1>Snippets</h1>
      <span class="sub">Comandos reutilizables que puedes enviar a una o varias sesiones a la vez.</span>
    </div>
    <div class="actions">
      <button onclick={add}><Icon name="plus" />Nuevo</button>
      <button class="primary" onclick={save} disabled={loading}><Icon name="save" />Guardar</button>
    </div>
  </header>

  {#if error}<p class="alert" role="alert"><Icon name="alert" />{error}</p>{/if}
  {#if notice}<p class="alert ok" role="status"><Icon name="check" />{notice}</p>{/if}

  <div class="layout">
    <ul class="list panel">
      {#each list as s, i (i)}
        <li>
          <button class:active={sel === i} aria-current={sel === i ? "true" : undefined} onclick={() => { sel = i; notice = null; }}>
            <span class="sico"><Icon name="braces" size={14} /></span>
            <span class="sname">{s.name || "(sin nombre)"}</span>
          </button>
        </li>
      {:else}
        <li class="dim">{loading ? "Cargando…" : "Sin snippets todavía"}</li>
      {/each}
    </ul>

    <div class="editor">
      {#if current && sel !== null}
        <div class="panel card">
          <div class="card-head">
            <label class="field grow">Nombre <input bind:value={list[sel].name} /></label>
            <button class="icon-btn danger-hover" onclick={remove} title="Eliminar snippet" aria-label="Eliminar"><Icon name="trash" /></button>
          </div>
          <label class="field">Comando <textarea rows="6" bind:value={list[sel].command} spellcheck="false"></textarea></label>
        </div>
        <SnippetRunner command={current.command} />
      {:else}
        <div class="empty-state">
          <span class="ico"><Icon name="braces" size={20} /></span>
          <h3>Ningún snippet seleccionado</h3>
          <p>Crea uno con <strong>Nuevo</strong> y guárdalo para tenerlo siempre a mano.</p>
        </div>
      {/if}
    </div>
  </div>
</section>

<style>
  .layout { display: grid; grid-template-columns: 232px minmax(0, 1fr); gap: 16px; align-items: start; }
  .list { list-style: none; margin: 0; padding: 4px; display: grid; gap: 1px; }
  .list li button { width: 100%; height: 32px; justify-content: flex-start; gap: 9px; padding: 0 10px; border: 0; background: none; font-weight: 400; }
  .list li button:hover:not(:disabled) { background: var(--bg-hover); }
  .list li button.active, .list li button.active:hover:not(:disabled) { background: var(--accent-soft); color: var(--accent); font-weight: 500; }
  .sico { display: flex; color: var(--fg-faint); }
  .active .sico { color: var(--accent); }
  .sname { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; font-family: var(--font-mono); font-size: 12.5px; }
  .dim { color: var(--fg-faint); padding: 10px; font-size: 12.5px; }
  .editor { display: flex; flex-direction: column; gap: 16px; min-width: 0; }
  .card { padding: 16px; display: grid; gap: 12px; }
  .card-head { display: flex; align-items: flex-end; gap: 8px; }
  .grow { flex: 1; }
  textarea { font-family: var(--font-mono); font-size: 12.5px; }
  @media (max-width: 720px) { .layout { grid-template-columns: 1fr; } }
</style>
