<script lang="ts">
  import { termWrite } from "../lib/api";
  import { errorMessage } from "../lib/errors";
  import { sessions } from "../lib/sessions.svelte";
  import { snippetBytes } from "../lib/snippet-logic";
  import type { TermId } from "../lib/types";
  import Icon from "./Icon.svelte";

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

<div class="runner panel">
  <h3>Enviar a sesiones</h3>
  {#if sessions.terms.length === 0}
    <p class="dim">No hay pestañas de terminal abiertas. Conecta un host para enviarle este snippet.</p>
  {:else}
    <ul>
      {#each sessions.terms as t (t.id)}
        <li>
          <label class="chip" class:on={picked.includes(t.id)}>
            <input type="checkbox" checked={picked.includes(t.id)} onchange={() => toggle(t.id)} />
            <span class="dot open" aria-hidden="true"></span>{t.label}
          </label>
        </li>
      {/each}
    </ul>
  {/if}
  <pre class="preview"><span class="prompt" aria-hidden="true">$ </span>{command || " "}</pre>
  <div class="foot">
    <label class="check"><input type="checkbox" bind:checked={newline} /> Añadir salto de línea (Enter)</label>
    {#if status}<span class="dim">{status}</span>{/if}
    <button class="primary" onclick={send} disabled={targets.length === 0 || command.trim() === "" || sending}>
      <Icon name="send" size={14} />{sending ? "Enviando…" : "Enviar"}
    </button>
  </div>
</div>

<style>
  .runner { display: flex; flex-direction: column; gap: 12px; padding: 16px; }
  h3 { font-size: 13.5px; }
  ul { list-style: none; margin: 0; padding: 0; display: flex; flex-wrap: wrap; gap: 6px; max-height: 160px; overflow: auto; }
  .chip { display: inline-flex; align-items: center; gap: 7px; height: 28px; padding: 0 10px 0 8px; border-radius: 14px; border: 1px solid var(--border-strong); font-size: 12.5px; cursor: pointer; transition: background-color 120ms var(--ease-out), border-color 120ms var(--ease-out); }
  .chip:hover { background: var(--bg-hover); }
  .chip.on { border-color: var(--accent-line); background: var(--accent-soft); color: var(--accent); }
  .chip:has(input:focus-visible) { outline: 2px solid var(--accent); outline-offset: 2px; }
  .chip input { width: 13px; height: 13px; }
  .preview { margin: 0; font-family: var(--font-mono); font-size: 12px; background: var(--term-bg); color: var(--term-fg); border: 1px solid var(--border); border-radius: var(--r-sm); padding: 8px 10px; max-height: 120px; overflow: auto; white-space: pre-wrap; }
  .prompt { color: var(--ok); }
  .foot { display: flex; align-items: center; gap: 12px; flex-wrap: wrap; }
  .foot button { margin-left: auto; }
  .dim { color: var(--fg-dim); margin: 0; font-size: 12.5px; }
</style>
