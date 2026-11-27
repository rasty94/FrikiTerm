<script lang="ts">
  import { tick } from "svelte";
  import { app } from "../lib/state.svelte";

  let okBtn = $state<HTMLButtonElement>();
  const c = $derived(app.confirm);
  $effect(() => {
    if (c) tick().then(() => okBtn?.focus());
  });

  function done(v: boolean) {
    const cur = app.confirm;
    app.confirm = null;
    cur?.resolve(v);
  }
</script>

{#if c}
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div class="scrim" onkeydown={(e) => e.key === "Escape" && done(false)}>
    <div class="modal" role="alertdialog" aria-modal="true" aria-labelledby="cm-text">
      <p id="cm-text">{c.text}</p>
      <div class="actions">
        <button onclick={() => done(false)}>Cancelar</button>
        <button class="primary" bind:this={okBtn} onclick={() => done(true)}>{c.ok}</button>
      </div>
    </div>
  </div>
{/if}

<style>
  .scrim { position: fixed; inset: 0; background: var(--scrim); display: grid; place-items: center; z-index: var(--z-modal); animation: fade 140ms var(--ease-out); }
  .modal { width: min(420px, calc(100vw - 32px)); box-sizing: border-box; background: var(--bg-raised); color: var(--fg); border: 1px solid var(--border-strong); border-radius: 12px; padding: 20px; display: grid; gap: 18px; box-shadow: var(--shadow-3); animation: pop 180ms var(--ease-out); }
  p { margin: 0; white-space: pre-wrap; overflow-wrap: anywhere; max-height: 40vh; overflow: auto; font-size: 13.5px; line-height: 1.5; }
  .actions { display: flex; justify-content: flex-end; gap: 8px; }
  @keyframes fade { from { opacity: 0; } }
  @keyframes pop { from { opacity: 0; transform: scale(0.97) translateY(4px); } }
</style>
