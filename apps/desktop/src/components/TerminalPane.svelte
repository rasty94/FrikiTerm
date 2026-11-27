<script lang="ts">
  import { onMount } from "svelte";
  import { Terminal } from "@xterm/xterm";
  import { FitAddon } from "@xterm/addon-fit";
  import { WebglAddon } from "@xterm/addon-webgl";
  import { SearchAddon } from "@xterm/addon-search";
  import { WebLinksAddon } from "@xterm/addon-web-links";
  import "@xterm/xterm/css/xterm.css";
  import { openUrl, termOpen, termWrite, termResize, termClose } from "../lib/api";
  import type { TermEvent, TermId } from "../lib/types";
  import { app, askConfirm, setActiveLeaf, tabById, toast, type Leaf } from "../lib/state.svelte";
  import { effectiveFontSize } from "../lib/settings";
  import { needsPasteConfirm } from "../lib/paste";
  import { isMac } from "../lib/shortcuts";
  import FindBar from "./FindBar.svelte";
  import ContextMenu from "./ContextMenu.svelte";
  import { errMsg } from "../lib/session";
  import { addConn, addTerm, removeTerm } from "../lib/sessions.svelte";

  let { leaf, tabId }: { leaf: Leaf; tabId: number } = $props();

  let host: HTMLDivElement;
  let term = $state.raw<Terminal>();
  let fit: FitAddon | undefined;
  let search: SearchAddon | undefined;
  let finding = $state(false);
  let findKey = $state(0);
  let seenFind = app.find.n;
  let menu = $state<{ x: number; y: number } | null>(null);
  const mac = isMac();

  const monoVar = () => getComputedStyle(document.documentElement).getPropertyValue("--font-mono");
  const fontFamily = () => (app.settings.fontFamily ? `${app.settings.fontFamily}, ${monoVar()}` : monoVar());

  /** Pegado con la regla de confirmación para varias líneas; `paste` respeta el modo bracketed de la shell. */
  async function pasteText(text: string) {
    if (!text) return;
    if (needsPasteConfirm(text) && !(await askConfirm(`Vas a pegar varias líneas; cada una se ejecutará:\n\n${text.slice(0, 600)}${text.length > 600 ? "…" : ""}`, "Pegar"))) return;
    term?.paste(text);
  }
  async function pasteFromClipboard() {
    try {
      await pasteText(await navigator.clipboard.readText());
    } catch {
      toast("No se pudo leer el portapapeles");
    }
    term?.focus();
  }
  function copySelection() {
    const sel = term?.getSelection();
    if (sel) navigator.clipboard.writeText(sel).catch(() => toast("No se pudo copiar"));
  }
  function closeFind() {
    finding = false;
    search?.clearDecorations();
    term?.focus();
  }
  function onContextMenu(e: MouseEvent) {
    e.preventDefault();
    if (app.settings.rightClick === "paste") void pasteFromClipboard();
    else menu = { x: e.clientX, y: e.clientY };
  }
  // Cmd/Ctrl+C copia solo con selección (si no, ^C); Ctrl+V / Ctrl+Shift+V dejan pasar el evento nativo `paste`.
  function onKey(e: KeyboardEvent): boolean {
    if (e.type !== "keydown" || e.altKey) return true;
    if (e.code === "KeyC" && (mac ? e.metaKey && !e.ctrlKey : e.ctrlKey)) {
      if (term?.hasSelection()) {
        copySelection();
        term.clearSelection();
        return false;
      }
      if (mac) {
        term?.input("\x03");
        return false;
      }
      return !e.shiftKey; // Ctrl+Shift+C sin selección: no envía nada
    }
    if (e.code === "KeyV" && !mac && e.ctrlKey) return false;
    return true;
  }

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

  // Ajustes en vivo.
  $effect(() => {
    const st = app.settings;
    const size = effectiveFontSize(st.fontSize, app.zoom);
    if (!term) return;
    term.options.fontFamily = fontFamily();
    term.options.fontSize = size;
    term.options.cursorStyle = st.cursorStyle;
    term.options.cursorBlink = st.cursorBlink;
    term.options.scrollback = st.scrollback;
    fit?.fit();
  });

  $effect(() => {
    const f = app.find;
    if (f.n !== seenFind) {
      seenFind = f.n;
      if (f.leaf === leaf.id) {
        finding = true;
        findKey++;
      }
    }
  });

  // El panel activo (clic, atajo o pestaña nueva) toma el foco del teclado.
  $effect(() => {
    if (active && app.view === "terminal" && app.activeTab === tabId && !finding) term?.focus();
  });

  onMount(() => {
    const st = app.settings;
    const t = new Terminal({
      cursorBlink: st.cursorBlink,
      cursorStyle: st.cursorStyle,
      fontSize: effectiveFontSize(st.fontSize, app.zoom),
      fontFamily: fontFamily(),
      scrollback: st.scrollback,
      theme: readTheme(),
    });
    fit = new FitAddon();
    t.loadAddon(fit);
    search = new SearchAddon();
    t.loadAddon(search);
    t.loadAddon(new WebLinksAddon((_e, uri) => void openUrl(uri).catch((err) => toast(errMsg(err)))));
    t.attachCustomKeyEventHandler(onKey);
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

    // Se intercepta en captura para poder pedir confirmación antes de que xterm procese el pegado.
    const onPaste = (ev: ClipboardEvent) => {
      const text = ev.clipboardData?.getData("text") ?? "";
      if (!needsPasteConfirm(text)) return;
      ev.preventDefault();
      ev.stopPropagation();
      void pasteText(text);
    };
    const onMouseUp = () => {
      if (app.settings.copyOnSelect && t.hasSelection()) copySelection();
    };
    host.addEventListener("paste", onPaste, true);
    host.addEventListener("mouseup", onMouseUp);

    let id: TermId | null = null;
    let closed = false;
    // Sesión terminada: se deja de enviar y la pestaña sale de sessions (SnippetRunner, etc.).
    let dead = false;
    const endSession = () => {
      dead = true;
      if (id !== null) removeTerm(id);
      id = null;
    };
    const sub = t.onData((d) => {
      if (id !== null) termWrite(id, new TextEncoder().encode(d)).catch((e) => toast(errMsg(e)));
    });
    const rs = t.onResize(({ cols, rows }) => {
      if (id !== null) termResize(id, cols, rows).catch(() => {});
    });
    const ro = new ResizeObserver(() => {
      if (host.clientWidth && host.clientHeight) fit?.fit();
    });
    ro.observe(host);

    leaf.status = { state: "connecting" };
    termOpen(
      leaf.target,
      t.cols,
      t.rows,
      (buf) => t.write(new Uint8Array(buf)),
      (ev) => {
        if (ev.type === "exit" || ev.type === "closed" || ev.type === "lost" || ev.type === "failed") endSession();
        onEvent(ev);
      },
    )
      .then((tid) => {
        if (closed || dead) return void termClose(tid).catch(() => {});
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
      host.removeEventListener("paste", onPaste, true);
      host.removeEventListener("mouseup", onMouseUp);
      endSession();
      ro.disconnect();
      sub.dispose();
      rs.dispose();
      t.dispose();
      term = undefined;
      fit = search = undefined;
      if (id !== null) {
        removeTerm(id);
        termClose(id).catch(() => {});
      }
    };
  });
</script>

<div class="pane" class:active onfocusin={() => setActiveLeaf(tabId, leaf.id)} role="group" aria-label="Terminal {leaf.title}">
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div class="term" bind:this={host} oncontextmenu={onContextMenu}></div>
  {#if finding}
    {#key findKey}
      <FindBar
        onfind={(q, dir, incremental) => (dir === 1 ? search?.findNext(q, { incremental }) : search?.findPrevious(q, { incremental })) ?? false}
        onclose={closeFind}
      />
    {/key}
  {/if}
  {#if menu}
    <ContextMenu
      x={menu.x}
      y={menu.y}
      onclose={() => { menu = null; term?.focus(); }}
      items={[
        ...(term?.hasSelection() ? [{ label: "Copiar", run: copySelection }] : []),
        { label: "Pegar", run: () => void pasteFromClipboard() },
        { label: "Buscar…", run: () => { finding = true; findKey++; } },
        { label: "Limpiar terminal", run: () => term?.clear() },
      ]}
    />
  {/if}
  <div class="status {leaf.status.state}" role="status">{statusText}</div>
</div>

<style>
  .pane { position: relative; display: flex; flex-direction: column; flex: 1; min-width: 0; min-height: 0; border: 1px solid var(--border); border-radius: 4px; overflow: hidden; }
  .pane.active { border-color: var(--accent); }
  .term { flex: 1; min-height: 0; padding: 4px; background: var(--term-bg); }
  .status { font-size: 12px; padding: 2px 8px; color: var(--fg-dim); border-top: 1px solid var(--border); white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
  .status.failed, .status.closed { color: var(--danger); }
</style>
