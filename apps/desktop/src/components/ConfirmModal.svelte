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
  .scrim { position: fixed; inset: 0; background: rgb(0 0 0 / 0.45); display: grid; place-items: center; z-index: 55; }
  .modal { width: min(440px, calc(100vw - 32px)); background: var(--bg-elev); color: var(--fg); border: 1px solid var(--border); border-radius: 10px; padding: 20px; display: grid; gap: 14px; }
  p { margin: 0; white-space: pre-wrap; overflow-wrap: anywhere; max-height: 40vh; overflow: auto; }
  .actions { display: flex; justify-content: flex-end; gap: 8px; }
</style>
