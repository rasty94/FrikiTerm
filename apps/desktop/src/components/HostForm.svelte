<script lang="ts">
  import { onMount, untrack } from "svelte";
  import { hostSave, pickIdentityFile, serialPorts } from "../lib/api";
  import type {
    BackspaceMode, Emulation, FlowControl, Forward, Parity, Protocol, SerialPortInfo, SessionConfig,
  } from "../lib/types";
  import { defaultSession, errMsg } from "../lib/session";
  import { normFolder } from "../lib/folderTree";
  import { toast } from "../lib/state.svelte";
  import {
    BAUD_RATES, DATA_BITS, STOP_BITS, hasErrors, isPort, parseIdentityText, validateForward, validateSession,
    type HostErrors,
  } from "../lib/hostForm";
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
    ["auth", "Autenticación"],
    ["terminal", "Terminal"],
    ["forwards", "Redirecciones"],
    ["serial", "Serie"],
  ];
  // Sin tn3270: el backend no lo admite en el escritorio.
  const PROTOCOLS: Protocol[] = ["ssh", "telnet", "telnet-tls", "raw", "serial", "local-shell"];
  const PROTOCOL_LABELS: Record<string, string> = {
    ssh: "SSH",
    telnet: "Telnet",
    "telnet-tls": "Telnet TLS",
    raw: "RAW",
    serial: "Puerto serie",
    "local-shell": "Shell local",
  };
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
  let ids = $state<string[]>([...start.identity_files]);
  let fwd = $state(start.forwards.map(toRow));
  let logPath = $state(start.log?.path ?? "");
  let logAppend = $state(start.log?.append ?? true);
  let logInput = $state(start.log?.log_input ?? false);
  let error = $state("");
  let saving = $state(false);
  let ports = $state<SerialPortInfo[]>([]);
  let portsError = $state("");

  const isSsh = $derived(draft.protocol === "ssh");
  const isSerial = $derived(draft.protocol === "serial");
  const isLocal = $derived(draft.protocol === "local-shell");
  const isNet = $derived(!isSerial && !isLocal);

  // Pestañas según el protocolo; si la activa desaparece (cambió el protocolo), vuelve a General.
  const sections = $derived(
    SECTIONS.filter(([id]) => (id === "auth" || id === "forwards" ? isSsh : id === "serial" ? isSerial : true)),
  );
  const current = $derived(sections.some(([id]) => id === section) ? section : "general");

  // Lo que se guardaría: mismas normalizaciones que antes (nombre y host recortados, ruta de log, etc.).
  const candidate = $derived.by((): SessionConfig => {
    const d = $state.snapshot(draft) as SessionConfig;
    return {
      ...d,
      name: d.name.trim(),
      host: d.host.trim(),
      port: isNet || isPort(d.port) ? d.port : 22,
      user: isSsh ? d.user?.trim() || null : null,
      folder: normFolder(d.folder),
      local_command: isLocal ? d.local_command?.trim() || null : null,
      identity_files: isSsh ? parseIdentityText(ids.join("\n")) : [],
      forwards: isSsh ? fwd.map(fromRow) : [],
      log: logPath.trim() ? { path: logPath.trim(), append: logAppend, log_input: logInput } : null,
    };
  });
  const errors: HostErrors = $derived(validateSession(candidate));
  const valid = $derived(!hasErrors(errors));

  // Pestañas con algún campo inválido (para marcarlas aunque no estén activas).
  const bad: Record<Section, boolean> = $derived({
    general: !!(errors.name || errors.host || errors.port),
    auth: false,
    terminal: false,
    forwards: !!errors.forwards,
    serial: !!(errors.device || errors.baud || errors.dataBits || errors.stopBits),
  });

  // Puertos serie: se piden solo cuando el protocolo es serie.
  let portsAsked = false;
  $effect(() => {
    if (isSerial && !portsAsked) {
      portsAsked = true;
      serialPorts()
        .then((p) => (ports = p))
        .catch((e) => (portsError = errMsg(e)));
    }
  });
  // Un dispositivo guardado que ya no aparece sigue en la lista para no perderlo al guardar.
  const portOptions = $derived.by((): SerialPortInfo[] => {
    const cur = draft.serial.device;
    if (!cur || ports.some((p) => p.name === cur)) return ports;
    return [{ name: cur, description: "no detectado ahora" }, ...ports];
  });

  function addForward() {
    fwd.push({ kind: "local", bind: "127.0.0.1", port: 2222, host: "localhost", host_port: 22 });
  }

  async function pickKey() {
    try {
      const p = await pickIdentityFile();
      if (p && !ids.includes(p)) ids.push(p);
    } catch (e) {
      toast(errMsg(e));
    }
  }

  async function save(e: SubmitEvent) {
    e.preventDefault();
    if (!valid || saving) return;
    saving = true;
    error = "";
    try {
      await hostSave(candidate, originalName);
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

{#snippet fe(msg: string | undefined)}
  {#if msg}<span class="fe">{msg}</span>{/if}
{/snippet}

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
    {#each sections as [id, label] (id)}
      <button
        role="tab"
        class:on={current === id}
        class:bad={bad[id]}
        aria-selected={current === id}
        aria-controls="hf-panel"
        onclick={() => (section = id)}
      >{label}</button>
    {/each}
  </div>

  <form onsubmit={save} novalidate>
    <div class="body" role="tabpanel" id="hf-panel">
      {#if current === "general"}
        <div class="row2">
          <div class="f">
            <label>Nombre <input bind:value={draft.name} required autocomplete="off" aria-invalid={errors.name ? "true" : undefined} /></label>
            {@render fe(errors.name)}
          </div>
          <div class="f">
            <label>Protocolo
              <select bind:value={draft.protocol}>
                {#each PROTOCOLS as p (p)}<option value={p}>{PROTOCOL_LABELS[p]}</option>{/each}
              </select>
            </label>
          </div>
        </div>

        {#if isNet}
          <div class="row2 host-row">
            <div class="f">
              <label>Host <input bind:value={draft.host} required autocomplete="off" aria-invalid={errors.host ? "true" : undefined} /></label>
              {@render fe(errors.host)}
            </div>
            <div class="f">
              <label>Puerto <input type="number" min="1" max="65535" bind:value={draft.port} aria-invalid={errors.port ? "true" : undefined} /></label>
              {@render fe(errors.port)}
            </div>
          </div>
        {/if}

        {#if isLocal}
          <label>Comando local (opcional) <input bind:value={draft.local_command} placeholder="Por defecto: el shell del usuario" /></label>
        {/if}

        <label>Carpeta
          <input list="hf-folders" bind:value={draft.folder} placeholder="produccion/web" autocomplete="off" />
        </label>
        <datalist id="hf-folders">
          {#each folders as f (f)}<option value={f}></option>{/each}
        </datalist>

        {#if isSsh}
          <label>ProxyJump <input bind:value={draft.proxy_jump} placeholder="usuario@salto" autocomplete="off" /></label>
          <div class="row2">
            <div class="f">
              <label>Timeout (s) <input type="number" min="1" bind:value={draft.connect_timeout_secs} aria-invalid={errors.timeout ? "true" : undefined} /></label>
              {@render fe(errors.timeout)}
            </div>
            <div class="f">
              <label>Keepalive (s) <input type="number" min="0" bind:value={draft.keepalive_secs} aria-invalid={errors.keepalive ? "true" : undefined} /></label>
              {@render fe(errors.keepalive)}
            </div>
          </div>
        {/if}
      {:else if current === "auth"}
        <label>Usuario <input bind:value={draft.user} autocomplete="off" placeholder="Por defecto: el de la sesión" /></label>

        <div class="f" role="group" aria-label="Archivos de identidad">
          <span class="lbl">Archivos de identidad</span>
          {#each ids as _, i (i)}
            <div class="id-row">
              <input bind:value={ids[i]} aria-label="Archivo de identidad {i + 1}" placeholder="~/.ssh/id_ed25519" autocomplete="off" />
              <button type="button" class="icon-btn danger-hover" aria-label="Quitar archivo {i + 1}" title="Quitar" onclick={() => ids.splice(i, 1)}><Icon name="x" /></button>
            </div>
          {:else}
            <p class="hint">Ninguno: se usan el agente SSH y las claves por defecto.</p>
          {/each}
          <div class="actions-row">
            <button type="button" class="add" onclick={pickKey}><Icon name="folder" />Elegir archivo…</button>
            <button type="button" class="add" onclick={() => ids.push("")}><Icon name="plus" />Escribir ruta</button>
          </div>
        </div>

        <div class="row2">
          <label>Intentos de autenticación <input type="number" min="1" bind:value={draft.max_auth_attempts} aria-invalid={errors.maxAuth ? "true" : undefined} /></label>
          <label>PKCS#11 provider <input bind:value={draft.pkcs11_provider} autocomplete="off" /></label>
        </div>
        {@render fe(errors.maxAuth)}
        <fieldset class="opts">
          <legend>Opciones SSH</legend>
          <label class="check"><input type="checkbox" bind:checked={draft.forward_agent} /> Reenviar agente SSH</label>
          <label class="check"><input type="checkbox" bind:checked={draft.require_strict_kex} /> Exigir strict KEX</label>
          <label class="check"><input type="checkbox" bind:checked={draft.gssapi} /> GSSAPI</label>
          <label class="check"><input type="checkbox" bind:checked={draft.gssapi_delegate} /> Delegar credenciales GSSAPI</label>
          <label class="check"><input type="checkbox" bind:checked={draft.legacy_algorithms} /> Algoritmos legacy</label>
        </fieldset>
        <p class="hint">Las contraseñas nunca se guardan: se piden al conectar.</p>
      {:else if current === "terminal"}
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
      {:else if current === "forwards"}
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
              <input bind:value={f.host} aria-label="Host destino" placeholder="localhost" />
              <input type="number" bind:value={f.host_port} aria-label="Puerto destino" />
            {:else}
              <span class="hint">SOCKS</span><span></span>
            {/if}
            <button type="button" class="icon-btn danger-hover" aria-label="Quitar redirección" title="Quitar" onclick={() => fwd.splice(i, 1)}><Icon name="x" /></button>
          </div>
          {@const ferr = validateForward(fromRow(f))}
          {#if ferr}<span class="fe">Redirección {i + 1}: {ferr}</span>{/if}
        {:else}
          <p class="hint">Sin redirecciones.</p>
        {/each}
        <button type="button" class="add" onclick={addForward}><Icon name="plus" />Redirección</button>
      {:else}
        <div class="f">
          <label>Dispositivo
            <select bind:value={draft.serial.device} aria-invalid={errors.device ? "true" : undefined}>
              <option value="">— elige un puerto —</option>
              {#each portOptions as p (p.name)}
                <option value={p.name}>{p.name}{p.description ? ` · ${p.description}` : ""}</option>
              {/each}
            </select>
          </label>
          {@render fe(errors.device)}
          {#if portsError}<span class="hint">No se pudieron listar los puertos ({portsError}).</span>{/if}
          {#if !portsError && ports.length === 0}<span class="hint">No se detecta ningún puerto serie conectado.</span>{/if}
        </div>
        <div class="row2">
          <div class="f">
            <label>Velocidad (baud)
              <input type="number" min="1" step="1" list="hf-bauds" bind:value={draft.serial.baud} aria-invalid={errors.baud ? "true" : undefined} />
            </label>
            <datalist id="hf-bauds">
              {#each BAUD_RATES as b (b)}<option value={b}></option>{/each}
            </datalist>
            {@render fe(errors.baud)}
          </div>
          <div class="f">
            <label>Bits de datos
              <select bind:value={draft.serial.data_bits} aria-invalid={errors.dataBits ? "true" : undefined}>
                {#each DATA_BITS as b (b)}<option value={b}>{b}</option>{/each}
              </select>
            </label>
            {@render fe(errors.dataBits)}
          </div>
        </div>
        <div class="row2">
          <label>Paridad
            <select bind:value={draft.serial.parity}>
              {#each PARITY as p (p)}<option value={p}>{p}</option>{/each}
            </select>
          </label>
          <div class="f">
            <label>Bits de parada
              <select bind:value={draft.serial.stop_bits} aria-invalid={errors.stopBits ? "true" : undefined}>
                {#each STOP_BITS as b (b)}<option value={b}>{b}</option>{/each}
              </select>
            </label>
            {@render fe(errors.stopBits)}
          </div>
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
      {#if !valid}<span class="need" aria-live="polite">Faltan datos válidos</span>{/if}
      <button type="button" onclick={onclose}>Cancelar</button>
      <button class="primary" disabled={!valid || saving}>Guardar</button>
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
  .tabs button { height: 36px; border: 0; border-radius: 0; background: none; padding: 0 8px; color: var(--fg-dim); box-shadow: inset 0 -2px 0 transparent; white-space: nowrap; }
  .tabs button:hover:not(:disabled) { background: none; color: var(--fg); }
  .tabs button.on { color: var(--fg); box-shadow: inset 0 -2px 0 var(--accent); }
  .tabs button.bad { color: var(--danger); }
  form { flex: 1; min-height: 0; display: flex; flex-direction: column; }
  .body { flex: 1; overflow: auto; padding: 20px; display: grid; gap: 14px; align-content: start; }
  label { display: grid; gap: 5px; font-size: 12px; font-weight: 500; color: var(--fg-dim); }
  label.check { display: flex; align-items: center; gap: 9px; font-size: 13px; font-weight: 400; color: var(--fg); cursor: pointer; }
  .lbl { font-size: 12px; font-weight: 500; color: var(--fg-dim); }
  .f { display: grid; gap: 5px; align-content: start; min-width: 0; }
  .fe { font-size: 12px; color: var(--danger); line-height: 1.35; }
  .need { font-size: 12px; color: var(--fg-dim); }
  .opts { display: grid; gap: 10px; margin: 4px 0 0; padding: 14px; border: 1px solid var(--border); border-radius: var(--r); }
  legend { padding: 0 6px; margin-left: -6px; font-size: 12px; font-weight: 500; color: var(--fg-dim); }
  .row2 { display: grid; grid-template-columns: 1fr 1fr; gap: 12px; }
  .row2 > * { min-width: 0; }
  .host-row { grid-template-columns: 1fr 120px; }
  .fwd { display: grid; grid-template-columns: 92px 1fr 72px 1fr 72px auto; gap: 6px; align-items: center; }
  .id-row { display: grid; grid-template-columns: 1fr auto; gap: 6px; align-items: center; }
  .actions-row { display: flex; flex-wrap: wrap; gap: 8px; }
  .add { justify-self: start; }
  .hint { font-size: 12px; color: var(--fg-dim); margin: 0; }
  .alert.err { margin: 0 20px 12px; }
  footer { display: flex; align-items: center; flex-wrap: wrap; gap: 8px; padding: 12px 16px 12px 20px; border-top: 1px solid var(--border); background: var(--bg-elev); }
  .del:hover:not(:disabled) { color: var(--danger); background: var(--danger-soft); }
  .grow { flex: 1; }
  @media (max-width: 520px) {
    .row2, .host-row, .fwd { grid-template-columns: 1fr; }
  }
  @keyframes fade { from { opacity: 0; } }
  @keyframes slide-in { from { transform: translateX(24px); opacity: 0; } }
</style>
