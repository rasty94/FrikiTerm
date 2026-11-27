<script lang="ts">
  import { untrack } from "svelte";
  import { hostSave } from "../lib/api";
  import type { BackspaceMode, Emulation, Forward, FlowControl, Parity, Protocol, SessionConfig } from "../lib/types";
  import { defaultSession, errMsg } from "../lib/session";
  import { normFolder } from "../lib/folderTree";
  import { toast } from "../lib/state.svelte";
  import Icon from "./Icon.svelte";

  let {
    host,
    folders,
    onclose,
    onsaved,
    ondelete,
  }: {
    host: SessionConfig | null;
    folders: string[];
    onclose: () => void;
    onsaved: () => void;
    ondelete?: (h: SessionConfig) => void;
  } = $props();

  type Section = "general" | "auth" | "terminal" | "forwards" | "serial";
  const SECTIONS: [Section, string][] = [
    ["general", "General"],
    ["auth", "Auth"],
    ["terminal", "Terminal"],
    ["forwards", "Forwards"],
    ["serial", "Serial"],
  ];
  const PROTOCOLS: Protocol[] = ["ssh", "telnet", "telnet-tls", "raw", "tn3270", "serial", "local-shell"];
  const EMULATIONS: Emulation[] = ["xterm", "vt100", "vt220", "linux", "ansi", "wyse50", "wyse60", "tvi910", "tvi925"];
  const BACKSPACE: BackspaceMode[] = ["delete", "ctrl-h"];
  const PARITY: Parity[] = ["none", "odd", "even"];
  const FLOW: FlowControl[] = ["none", "software", "hardware"];

  // Borrador: el host original no cambia hasta que se guarda.
  // untrack: se copia una vez al abrir; cambios posteriores del prop no reescriben el borrador.
  const originalName = untrack(() => host?.name ?? null);
  const start: SessionConfig = untrack(() =>
    host ? structuredClone($state.snapshot(host)) : defaultSession({ host: "" }),
  );
  let draft = $state(start);

  interface FwdRow {
    kind: Forward["kind"];
    bind: string;
    port: number;
    host: string;
    host_port: number;
  }
  const toRow = (f: Forward): FwdRow => ({
    kind: f.kind,
    bind: f.bind,
    port: f.port,
    host: "host" in f ? f.host : "",
    host_port: "host_port" in f ? f.host_port : 0,
  });
  const fromRow = (r: FwdRow): Forward =>
    r.kind === "dynamic"
      ? { kind: "dynamic", bind: r.bind, port: r.port }
      : { kind: r.kind, bind: r.bind, port: r.port, host: r.host, host_port: r.host_port };

  let section = $state<Section>("general");
  let idText = $state(start.identity_files.join("\n"));
  let fwd = $state(start.forwards.map(toRow));
  let logPath = $state(start.log?.path ?? "");
  let logAppend = $state(start.log?.append ?? true);
  let logInput = $state(start.log?.log_input ?? false);
  let error = $state("");
  let saving = $state(false);

  function addForward() {
    fwd.push({ kind: "local", bind: "127.0.0.1", port: 2222, host: "localhost", host_port: 22 });
  }

  async function save(e: SubmitEvent) {
    e.preventDefault();
    const name = draft.name.trim();
    const addr = draft.host.trim();
    if (!name) return (error = "Falta el nombre");
    if (!addr) return (error = "Falta el host");
    if (!Number.isInteger(draft.port) || draft.port < 1 || draft.port > 65535) return (error = "Puerto no válido");

    const payload: SessionConfig = {
      ...$state.snapshot(draft),
      name,
      host: addr,
      user: draft.user?.trim() || null,
      folder: normFolder(draft.folder),
      identity_files: idText.split("\n").map((s) => s.trim()).filter(Boolean),
      forwards: fwd.map(fromRow),
      log: logPath.trim() ? { path: logPath.trim(), append: logAppend, log_input: logInput } : null,
    };
    saving = true;
    error = "";
    try {
      await hostSave(payload, originalName);
      toast("Host guardado", "info");
      onsaved();
    } catch (err) {
      error = errMsg(err);
    } finally {
      saving = false;
    }
  }
</script>

<svelte:window onkeydown={(e) => e.key === "Escape" && onclose()} />

<div class="scrim" onclick={onclose} role="presentation"></div>
<div class="drawer" role="dialog" aria-modal="true" aria-labelledby="hf-title">
  <header>
    <div>
      <h2 id="hf-title">{host ? "Editar host" : "Nuevo host"}</h2>
      {#if host}<p class="sub">{host.name}</p>{/if}
    </div>
    <button class="icon-btn" aria-label="Cerrar" title="Cerrar (Esc)" onclick={onclose}><Icon name="x" /></button>
  </header>

  <div class="tabs" role="tablist">
    {#each SECTIONS as [id, label] (id)}
      <button role="tab" class:on={section === id} aria-selected={section === id} aria-controls="hf-panel" onclick={() => (section = id)}>{label}</button>
    {/each}
  </div>

  <form onsubmit={save}>
    <div class="body" role="tabpanel" id="hf-panel">
      {#if section === "general"}
        <label>Nombre <input bind:value={draft.name} required /></label>
        <label>Host <input bind:value={draft.host} required autocomplete="off" /></label>
        <div class="row2">
          <label>Puerto <input type="number" min="1" max="65535" bind:value={draft.port} /></label>
          <label>Protocolo
            <select bind:value={draft.protocol}>
              {#each PROTOCOLS as p (p)}<option value={p}>{p}</option>{/each}
            </select>
          </label>
        </div>
        {#if draft.protocol === "local-shell"}
          <label>Comando local <input bind:value={draft.local_command} /></label>
        {/if}
        <label>Carpeta
          <input list="hf-folders" bind:value={draft.folder} placeholder="produccion/web" />
        </label>
        <datalist id="hf-folders">
          {#each folders as f (f)}<option value={f}></option>{/each}
        </datalist>
        <label>ProxyJump <input bind:value={draft.proxy_jump} placeholder="usuario@salto" /></label>
        <div class="row2">
          <label>Timeout (s) <input type="number" min="1" bind:value={draft.connect_timeout_secs} /></label>
          <label>Keepalive (s) <input type="number" min="0" bind:value={draft.keepalive_secs} /></label>
        </div>
        <label>Intentos de autenticación <input type="number" min="1" bind:value={draft.max_auth_attempts} /></label>
        <label>PKCS#11 provider <input bind:value={draft.pkcs11_provider} /></label>
        <fieldset class="opts">
        <legend>Opciones SSH</legend>
        <label class="check"><input type="checkbox" bind:checked={draft.forward_agent} /> Reenviar agente SSH</label>
        <label class="check"><input type="checkbox" bind:checked={draft.gssapi} /> GSSAPI</label>
        <label class="check"><input type="checkbox" bind:checked={draft.gssapi_delegate} /> Delegar credenciales GSSAPI</label>
        <label class="check"><input type="checkbox" bind:checked={draft.legacy_algorithms} /> Algoritmos legacy</label>
        <label class="check"><input type="checkbox" bind:checked={draft.require_strict_kex} /> Exigir strict KEX</label>
        </fieldset>
      {:else if section === "auth"}
        <label>Usuario <input bind:value={draft.user} autocomplete="off" /></label>
        <label>Archivos de identidad (uno por línea)
          <textarea rows="5" bind:value={idText} placeholder="~/.ssh/id_ed25519"></textarea>
        </label>
        <p class="hint">Las contraseñas nunca se guardan: se piden al conectar.</p>
      {:else if section === "terminal"}
        <label>TERM <input bind:value={draft.term} /></label>
        <div class="row2">
          <label>Emulación
            <select bind:value={draft.terminal.emulation}>
              {#each EMULATIONS as e (e)}<option value={e}>{e}</option>{/each}
            </select>
          </label>
          <label>Retroceso
            <select bind:value={draft.terminal.backspace}>
              {#each BACKSPACE as b (b)}<option value={b}>{b}</option>{/each}
            </select>
          </label>
        </div>
        <div class="row2">
          <label>Scrollback <input type="number" min="0" bind:value={draft.terminal.scrollback} /></label>
          <label>Tema <input bind:value={draft.terminal.theme} /></label>
        </div>
        <label>Log de sesión (ruta) <input bind:value={logPath} placeholder="vacío = sin log" /></label>
        <label class="check"><input type="checkbox" bind:checked={logAppend} /> Añadir al log existente</label>
        <label class="check"><input type="checkbox" bind:checked={logInput} /> Registrar también la entrada</label>
      {:else if section === "forwards"}
        {#each fwd as f, i (i)}
          <div class="fwd">
            <select bind:value={f.kind} aria-label="Tipo de redirección">
              <option value="local">local</option>
              <option value="remote">remote</option>
              <option value="dynamic">dynamic</option>
            </select>
            <input bind:value={f.bind} aria-label="Dirección de escucha" placeholder="127.0.0.1" />
            <input type="number" bind:value={f.port} aria-label="Puerto de escucha" />
            {#if f.kind !== "dynamic"}
              <input bind:value={f.host} aria-label="Host destino" />
              <input type="number" bind:value={f.host_port} aria-label="Puerto destino" />
            {/if}
            <button type="button" class="icon-btn danger-hover" aria-label="Quitar redirección" title="Quitar" onclick={() => fwd.splice(i, 1)}><Icon name="x" /></button>
          </div>
        {:else}
          <p class="hint">Sin redirecciones.</p>
        {/each}
        <button type="button" class="add" onclick={addForward}><Icon name="plus" />Redirección</button>
      {:else}
        <label>Dispositivo <input bind:value={draft.serial.device} placeholder="/dev/ttyUSB0" /></label>
        <div class="row2">
          <label>Velocidad (baud) <input type="number" bind:value={draft.serial.baud} /></label>
          <label>Bits de datos <input type="number" bind:value={draft.serial.data_bits} /></label>
        </div>
        <div class="row2">
          <label>Paridad
            <select bind:value={draft.serial.parity}>
              {#each PARITY as p (p)}<option value={p}>{p}</option>{/each}
            </select>
          </label>
          <label>Bits de parada <input type="number" bind:value={draft.serial.stop_bits} /></label>
        </div>
        <label>Control de flujo
          <select bind:value={draft.serial.flow}>
            {#each FLOW as f (f)}<option value={f}>{f}</option>{/each}
          </select>
        </label>
      {/if}
    </div>

    {#if error}<p class="alert err" role="alert"><Icon name="alert" />{error}</p>{/if}

    <footer>
      {#if host && ondelete}<button type="button" class="ghost del" onclick={() => ondelete?.(host)}><Icon name="trash" />Eliminar</button>{/if}
      <span class="grow"></span>
      <button type="button" onclick={onclose}>Cancelar</button>
      <button class="primary" disabled={saving}>Guardar</button>
    </footer>
  </form>
</div>

<style>
  .scrim { position: fixed; inset: 0; background: var(--scrim); z-index: var(--z-drawer); animation: fade 160ms var(--ease-out); }
  .drawer {
    position: fixed; top: 0; right: 0; bottom: 0; width: min(480px, 100vw); z-index: calc(var(--z-drawer) + 1);
    display: flex; flex-direction: column; background: var(--bg-raised); border-left: 1px solid var(--border-strong); box-shadow: var(--shadow-3);
    animation: slide-in 220ms var(--ease-out);
  }
  header { display: flex; align-items: flex-start; justify-content: space-between; padding: 18px 16px 12px 20px; }
  h2 { font-size: 16px; }
  .sub { margin: 2px 0 0; color: var(--fg-dim); font-family: var(--font-mono); font-size: 12px; }
  .tabs { display: flex; gap: 4px; padding: 0 16px; border-bottom: 1px solid var(--border); overflow-x: auto; scrollbar-width: none; }
  .tabs button { height: 36px; border: 0; border-radius: 0; background: none; padding: 0 8px; color: var(--fg-dim); box-shadow: inset 0 -2px 0 transparent; }
  .tabs button:hover:not(:disabled) { background: none; color: var(--fg); }
  .tabs button.on { color: var(--fg); box-shadow: inset 0 -2px 0 var(--accent); }
  form { flex: 1; min-height: 0; display: flex; flex-direction: column; }
  .body { flex: 1; overflow: auto; padding: 20px; display: grid; gap: 14px; align-content: start; }
  label { display: grid; gap: 5px; font-size: 12px; font-weight: 500; color: var(--fg-dim); }
  label.check { display: flex; align-items: center; gap: 9px; font-size: 13px; font-weight: 400; color: var(--fg); cursor: pointer; }
  .opts { display: grid; gap: 10px; margin: 4px 0 0; padding: 14px; border: 1px solid var(--border); border-radius: var(--r); }
  legend { padding: 0 6px; margin-left: -6px; font-size: 12px; font-weight: 500; color: var(--fg-dim); }
  .row2 { display: grid; grid-template-columns: 1fr 1fr; gap: 12px; }
  .fwd { display: grid; grid-template-columns: 92px 1fr 72px 1fr 72px auto; gap: 6px; align-items: center; }
  .add { justify-self: start; }
  .hint { font-size: 12px; color: var(--fg-dim); margin: 0; }
  .err { margin: 0 20px 12px; }
  footer { display: flex; align-items: center; gap: 8px; padding: 12px 16px 12px 20px; border-top: 1px solid var(--border); background: var(--bg-elev); }
  .del:hover:not(:disabled) { color: var(--danger); background: var(--danger-soft); }
  .grow { flex: 1; }
  @keyframes fade { from { opacity: 0; } }
  @keyframes slide-in { from { transform: translateX(24px); opacity: 0; } }
</style>
