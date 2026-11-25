<script lang="ts">
  import { termWrite } from "../lib/api";
  import { errorMessage } from "../lib/errors";
  import { sessions } from "../lib/sessions.svelte";
  import { snippetBytes } from "../lib/snippet-logic";
  import type { TermId } from "../lib/types";

  interface Props {
    command: string;
  }
  let { command }: Props = $props();

  let picked = $state<TermId[]>([]);
  // Sólo sesiones vivas: una pestaña cerrada deja de figurar aunque estuviera marcada.
  const targets = $derived(picked.filter((id) => sessions.terms.some((t) => t.id === id)));
  let newline = $state(true);
  let status = $state<string | null>(null);
  let sending = $state(false);

  function toggle(id: TermId) {
    picked = picked.includes(id) ? picked.filter((x) => x !== id) : [...picked, id];
  }

  async function send() {
    if (targets.length === 0 || command.trim() === "") return;
    sending = true;
    status = null;
    const bytes = snippetBytes(command, newline);
    const results = await Promise.allSettled(targets.map((id) => termWrite(id, bytes)));
    const failed = results.filter((r): r is PromiseRejectedResult => r.status === "rejected");
    status = failed.length === 0
      ? `Enviado a ${targets.length} sesión(es)`
      : `Fallos: ${failed.length} de ${targets.length} (${errorMessage(failed[0].reason)})`;
    sending = false;
  }
</script>

<div class="runner">
  <h3>Enviar a sesiones</h3>
  {#if sessions.terms.length === 0}
    <p class="dim">No hay pestañas de terminal abiertas.</p>
  {:else}
    <ul>
      {#each sessions.terms as t (t.id)}
        <li>
          <label><input type="checkbox" checked={picked.includes(t.id)} onchange={() => toggle(t.id)} /> {t.label}</label>
        </li>
      {/each}
    </ul>
  {/if}
  <label class="nl"><input type="checkbox" bind:checked={newline} /> Añadir salto de línea (Enter)</label>
  <pre class="preview">{command || " "}</pre>
  <button onclick={send} disabled={targets.length === 0 || command.trim() === "" || sending}>
    {sending ? "Enviando…" : "Enviar"}
  </button>
  {#if status}<p class="dim">{status}</p>{/if}
</div>

<style>
  .runner { display: flex; flex-direction: column; gap: 8px; background: var(--bg-elev); border: 1px solid var(--border); border-radius: 8px; padding: 12px; }
  h3 { margin: 0; }
  ul { list-style: none; margin: 0; padding: 0; max-height: 160px; overflow: auto; }
  label { font-size: 13px; color: var(--fg); }
  .nl { color: var(--fg-dim); }
  .preview { margin: 0; font-family: var(--font-mono); font-size: 12px; background: var(--bg); border: 1px solid var(--border); border-radius: 6px; padding: 6px; max-height: 120px; overflow: auto; white-space: pre-wrap; }
  .dim { color: var(--fg-dim); margin: 0; font-size: 13px; }
  button { font: inherit; color: var(--fg); background: var(--bg); border: 1px solid var(--border); border-radius: 6px; padding: 5px 12px; cursor: pointer; align-self: flex-start; }
  button:disabled { opacity: 0.5; cursor: default; }
</style>
