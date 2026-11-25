<script lang="ts">
  import { onMount } from "svelte";
  import SnippetRunner from "../components/SnippetRunner.svelte";
  import { snippetsList, snippetsSave } from "../lib/api";
  import { errorMessage } from "../lib/errors";
  import { validateSnippets } from "../lib/snippet-logic";
  import type { Snippet } from "../lib/types";

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
  <header class="head">
    <h2>Snippets</h2>
    <div class="row">
      <button onclick={add}>Nuevo</button>
      <button onclick={remove} disabled={sel === null}>Eliminar</button>
      <button onclick={save} disabled={loading}>Guardar</button>
    </div>
  </header>

  {#if error}<p class="error">{error}</p>{/if}
  {#if notice}<p class="notice">{notice}</p>{/if}

  <div class="layout">
    <ul class="list">
      {#each list as s, i (i)}
        <li>
          <button class:active={sel === i} onclick={() => { sel = i; notice = null; }}>{s.name || "(sin nombre)"}</button>
        </li>
      {:else}
        <li class="dim">{loading ? "Cargando…" : "Sin snippets"}</li>
      {/each}
    </ul>

    <div class="editor">
      {#if current && sel !== null}
        <label>Nombre <input bind:value={list[sel].name} /></label>
        <label>Comando <textarea rows="6" bind:value={list[sel].command} spellcheck="false"></textarea></label>
        <SnippetRunner command={current.command} />
      {:else}
        <p class="dim">Selecciona o crea un snippet.</p>
      {/if}
    </div>
  </div>
</section>

<style>
  .view { display: flex; flex-direction: column; gap: 12px; }
  .head { display: flex; justify-content: space-between; align-items: center; }
  h2 { margin: 0; }
  .row { display: flex; gap: 8px; }
  .layout { display: grid; grid-template-columns: 220px 1fr; gap: 12px; align-items: start; }
  .list { list-style: none; margin: 0; padding: 0; background: var(--bg-elev); border: 1px solid var(--border); border-radius: 8px; overflow: hidden; }
  .list li button { width: 100%; text-align: left; background: none; border: 0; border-bottom: 1px solid var(--border); color: var(--fg); padding: 8px 12px; font: inherit; cursor: pointer; }
  .list li button.active { background: var(--term-selection); }
  .editor { display: flex; flex-direction: column; gap: 10px; min-width: 0; }
  label { display: flex; flex-direction: column; gap: 4px; font-size: 13px; color: var(--fg-dim); }
  input, textarea { font: inherit; color: var(--fg); background: var(--bg-elev); border: 1px solid var(--border); border-radius: 6px; padding: 6px 8px; }
  textarea { font-family: var(--font-mono); font-size: 13px; resize: vertical; }
  button { font: inherit; color: var(--fg); background: var(--bg-elev); border: 1px solid var(--border); border-radius: 6px; padding: 5px 12px; cursor: pointer; }
  button:disabled { opacity: 0.5; cursor: default; }
  .dim { color: var(--fg-dim); }
  .error { color: var(--danger); margin: 0; }
  .notice { color: var(--accent); margin: 0; }
  @media (max-width: 720px) { .layout { grid-template-columns: 1fr; } }
</style>
