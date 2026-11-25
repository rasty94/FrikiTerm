<script lang="ts">
  import { onMount } from "svelte";
  import { Terminal } from "@xterm/xterm";
  import { FitAddon } from "@xterm/addon-fit";
  import { WebglAddon } from "@xterm/addon-webgl";
  import "@xterm/xterm/css/xterm.css";
  import { termOpen, termWrite, termResize, termClose } from "../lib/api";
  import type { TermEvent, TermId } from "../lib/types";
  import { app, setActiveLeaf, tabById, toast, type Leaf } from "../lib/state.svelte";
  import { errMsg } from "../lib/session";
  import { addConn, addTerm, removeTerm } from "../lib/sessions.svelte";

  let { leaf, tabId }: { leaf: Leaf; tabId: number } = $props();

  let host: HTMLDivElement;
  let term: Terminal | undefined;

  const active = $derived(tabById(tabId)?.activeLeaf === leaf.id);

  const statusText = $derived.by(() => {
    const s = leaf.status;
    if (s.state === "connecting") return "Conectando…";
    if (s.state === "open") return [`Negociado: ${s.negotiated ?? "—"}`, ...s.warnings].join(" · ");
    return s.text;
  });

  /** Colores de xterm desde las variables CSS del tema activo. */
  function readTheme() {
    const s = getComputedStyle(document.documentElement);
    const v = (n: string) => s.getPropertyValue(n).trim();
    return {
      background: v("--term-bg"),
      foreground: v("--term-fg"),
      cursor: v("--term-cursor"),
      selectionBackground: v("--term-selection"),
    };
  }

  function onEvent(ev: TermEvent) {
    switch (ev.type) {
      case "opened":
        leaf.status = { state: "open", negotiated: ev.negotiated, warnings: ev.warnings };
        // Las vistas de SFTP, llavero y redirecciones reutilizan esta conexión.
        if (ev.conn !== null) addConn(ev.conn, leaf.title);
        break;
      case "failed":
        leaf.status = { state: "failed", text: ev.error.message };
        toast(ev.error.message);
        break;
      case "exit":
        leaf.status = {
          state: "closed",
          text: `Proceso terminado${ev.code === null ? "" : ` (código ${ev.code})`}`,
        };
        break;
      case "closed":
        leaf.status = { state: "closed", text: "Sesión cerrada" };
        break;
      case "lost":
        leaf.status = { state: "closed", text: `Conexión perdida${ev.reason ? `: ${ev.reason}` : ""}` };
        break;
    }
  }

  $effect(() => {
    void app.theme;
    if (term) term.options.theme = readTheme();
  });

  onMount(() => {
    const t = new Terminal({
      cursorBlink: true,
      fontSize: 13,
      fontFamily: getComputedStyle(document.documentElement).getPropertyValue("--font-mono"),
      scrollback: 10000,
      theme: readTheme(),
    });
    const fit = new FitAddon();
    t.loadAddon(fit);
    t.open(host);
    try {
      // Si el contexto WebGL se pierde se libera el addon y xterm vuelve al renderer DOM.
      const gl = new WebglAddon();
      gl.onContextLoss(() => gl.dispose());
      t.loadAddon(gl);
    } catch {
      /* sin WebGL2: renderer DOM */
    }
    fit.fit();
    term = t;

    let id: TermId | null = null;
    let closed = false;
    const sub = t.onData((d) => {
      if (id !== null) termWrite(id, new TextEncoder().encode(d)).catch((e) => toast(errMsg(e)));
    });
    const rs = t.onResize(({ cols, rows }) => {
      if (id !== null) termResize(id, cols, rows).catch(() => {});
    });
    const ro = new ResizeObserver(() => {
      if (host.clientWidth && host.clientHeight) fit.fit();
    });
    ro.observe(host);

    leaf.status = { state: "connecting" };
    termOpen(
      leaf.target,
      t.cols,
      t.rows,
      (buf) => t.write(new Uint8Array(buf)),
      onEvent,
    )
      .then((tid) => {
        if (closed) return void termClose(tid).catch(() => {});
        id = tid;
        addTerm({ id: tid, label: leaf.title });
        // Por si el panel cambió de tamaño mientras conectaba.
        termResize(tid, t.cols, t.rows).catch(() => {});
      })
      .catch((e) => {
        leaf.status = { state: "failed", text: errMsg(e) };
        toast(errMsg(e));
      });

    return () => {
      closed = true;
      ro.disconnect();
      sub.dispose();
      rs.dispose();
      t.dispose();
      term = undefined;
      if (id !== null) {
        removeTerm(id);
        termClose(id).catch(() => {});
      }
    };
  });
</script>

<div class="pane" class:active onfocusin={() => setActiveLeaf(tabId, leaf.id)} role="group" aria-label="Terminal {leaf.title}">
  <div class="term" bind:this={host}></div>
  <div class="status {leaf.status.state}" role="status">{statusText}</div>
</div>

<style>
  .pane { display: flex; flex-direction: column; flex: 1; min-width: 0; min-height: 0; border: 1px solid var(--border); border-radius: 4px; overflow: hidden; }
  .pane.active { border-color: var(--accent); }
  .term { flex: 1; min-height: 0; padding: 4px; background: var(--term-bg); }
  .status { font-size: 12px; padding: 2px 8px; color: var(--fg-dim); border-top: 1px solid var(--border); white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
  .status.failed, .status.closed { color: var(--danger); }
</style>
