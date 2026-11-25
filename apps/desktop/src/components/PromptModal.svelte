<script lang="ts">
  import { tick } from "svelte";
  import { app, toast } from "../lib/state.svelte";
  import { promptAnswer } from "../lib/api";
  import type { PromptAnswer } from "../lib/types";
  import { errMsg } from "../lib/session";

  const cur = $derived(app.prompts[0]);
  let secret = $state("");
  let remember = $state(false);
  let values = $state<string[]>([]);
  let dialog = $state<HTMLDivElement>();

  // Cada prompt nuevo empieza limpio y con foco dentro del modal.
  $effect(() => {
    const p = cur?.prompt;
    secret = "";
    remember = false;
    values = p?.kind === "kbdInteractive" ? p.prompts.map(() => "") : [];
    if (p) tick().then(() => dialog?.querySelector<HTMLElement>("input, button")?.focus());
  });

  function send(a: PromptAnswer) {
    const p = app.prompts.shift();
    if (!p) return;
    promptAnswer(p.id, a).catch((e) => toast(errMsg(e)));
  }

  /** Cancelar equivale a rechazar (host) o a no responder (secretos). */
  function cancel() {
    const p = cur?.prompt;
    if (!p) return;
    if (p.kind === "hostKeyUnknown" || p.kind === "hostKeyChanged") send({ kind: "hostKey", decision: "reject" });
    else if (p.kind === "kbdInteractive") send({ kind: "multi", values: null });
    else send({ kind: "secret", value: null, remember: false });
  }

  function submitSecret(e: SubmitEvent) {
    e.preventDefault();
    send({ kind: "secret", value: secret, remember });
  }

  function submitMulti(e: SubmitEvent) {
    e.preventDefault();
    send({ kind: "multi", values: [...values] });
  }

  /** Focus trap: Tab y Shift+Tab no salen del diálogo; Escape cancela. */
  function trap(e: KeyboardEvent) {
    if (e.key === "Escape") {
      e.preventDefault();
      cancel();
      return;
    }
    if (e.key !== "Tab" || !dialog) return;
    const els = [...dialog.querySelectorAll<HTMLElement>("input, button, select, textarea")].filter(
      (el) => !el.hasAttribute("disabled"),
    );
    if (els.length === 0) return;
    const first = els[0];
    const last = els[els.length - 1];
    if (e.shiftKey && document.activeElement === first) {
      e.preventDefault();
      last.focus();
    } else if (!e.shiftKey && document.activeElement === last) {
      e.preventDefault();
      first.focus();
    }
  }
</script>

{#if cur}
  {@const p = cur.prompt}
  <div class="scrim">
    <div class="modal" role="dialog" tabindex="-1" aria-modal="true" aria-labelledby="pm-title" bind:this={dialog} onkeydown={trap}>
      {#if p.kind === "hostKeyUnknown"}
        <h2 id="pm-title">Host desconocido</h2>
        <p>El servidor <strong>{p.host}:{p.port}</strong> no figura en known_hosts ({p.algorithm}). Compruebe la huella antes de aceptar.</p>
        <code class="fp">{p.fingerprint}</code>
        <div class="actions">
          <button type="button" onclick={cancel}>Rechazar</button>
          <button type="button" onclick={() => send({ kind: "hostKey", decision: "acceptOnce" })}>Aceptar una vez</button>
          <button type="button" class="primary" onclick={() => send({ kind: "hostKey", decision: "acceptAndSave" })}>Aceptar y guardar</button>
        </div>
      {:else if p.kind === "hostKeyChanged"}
        <h2 id="pm-title" class="danger-title">La clave del servidor ha CAMBIADO</h2>
        <p class="warn">Puede tratarse de un ataque de intermediario. No acepte salvo que sepa que el servidor fue reinstalado o su clave rotada.</p>
        <p><strong>{p.host}:{p.port}</strong> ({p.algorithm})</p>
        <p>Huella anterior</p>
        <code class="fp">{p.previousFingerprint}</code>
        <p>Huella nueva</p>
        <code class="fp">{p.fingerprint}</code>
        <div class="actions">
          <button type="button" class="primary" onclick={cancel}>Rechazar</button>
          <button type="button" class="danger" onclick={() => send({ kind: "hostKey", decision: "acceptOnce" })}>Aceptar una vez</button>
        </div>
      {:else if p.kind === "password"}
        <form onsubmit={submitSecret}>
          <h2 id="pm-title">Contraseña</h2>
          <p>{p.user}@{p.host}</p>
          <label class="field">Contraseña
            <input type="password" bind:value={secret} autocomplete="off" />
          </label>
          <label class="check"><input type="checkbox" bind:checked={remember} /> Recordar</label>
          <div class="actions">
            <button type="button" onclick={cancel}>Cancelar</button>
            <button class="primary">Aceptar</button>
          </div>
        </form>
      {:else if p.kind === "passphrase"}
        <form onsubmit={submitSecret}>
          <h2 id="pm-title">Passphrase de la clave</h2>
          <p><code>{p.keyPath}</code></p>
          <label class="field">Passphrase
            <input type="password" bind:value={secret} autocomplete="off" />
          </label>
          <label class="check"><input type="checkbox" bind:checked={remember} /> Recordar</label>
          <div class="actions">
            <button type="button" onclick={cancel}>Cancelar</button>
            <button class="primary">Aceptar</button>
          </div>
        </form>
      {:else}
        <form onsubmit={submitMulti}>
          <h2 id="pm-title">{p.name || "Autenticación interactiva"}</h2>
          {#if p.instructions}<p>{p.instructions}</p>{/if}
          {#each p.prompts as q, i (i)}
            <label class="field">{q.text}
              <input type={q.echo ? "text" : "password"} bind:value={values[i]} autocomplete="off" />
            </label>
          {/each}
          <div class="actions">
            <button type="button" onclick={cancel}>Cancelar</button>
            <button class="primary">Enviar</button>
          </div>
        </form>
      {/if}
    </div>
  </div>
{/if}

<style>
  .scrim { position: fixed; inset: 0; background: rgb(0 0 0 / 0.45); display: grid; place-items: center; z-index: 50; }
  .modal { width: min(480px, calc(100vw - 32px)); background: var(--bg-elev); color: var(--fg); border: 1px solid var(--border); border-radius: 10px; padding: 20px; display: grid; gap: 10px; }
  h2 { margin: 0; font-size: 17px; }
  .danger-title, .warn { color: var(--danger); }
  p { margin: 0; color: var(--fg-dim); overflow-wrap: anywhere; }
  .fp { font-family: var(--font-mono); font-size: 12px; background: var(--bg); padding: 8px; border-radius: 6px; word-break: break-all; color: var(--fg); }
  form { display: grid; gap: 10px; }
  .field { display: grid; gap: 4px; color: var(--fg-dim); font-size: 12px; }
  .check { display: flex; align-items: center; gap: 6px; color: var(--fg-dim); }
  .actions { display: flex; justify-content: flex-end; gap: 8px; flex-wrap: wrap; }
</style>
