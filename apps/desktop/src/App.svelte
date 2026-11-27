<script lang="ts">
  import { onMount } from "svelte";
  import { appSubscribe } from "./lib/api";
  import { app, cycleTab, focusNextPane, loadSettings, requestCloseTab, splitPane, tabById } from "./lib/state.svelte";
  import { FONT_MAX, FONT_MIN } from "./lib/settings";
  import { isMac, matchShortcut, type ShortcutAction } from "./lib/shortcuts";
  import { handleAppEvent } from "./lib/sessions.svelte";
  import NavRail from "./components/NavRail.svelte";
  import PromptModal from "./components/PromptModal.svelte";
  import ConfirmModal from "./components/ConfirmModal.svelte";
  import HostsView from "./views/HostsView.svelte";
  import Workspace from "./views/Workspace.svelte";
  import SftpView from "./views/SftpView.svelte";
  import KeychainView from "./views/KeychainView.svelte";
  import SnippetsView from "./views/SnippetsView.svelte";
  import ForwardsView from "./views/ForwardsView.svelte";
  import SettingsView from "./views/SettingsView.svelte";

  // promptDone aún no está en AppEvent (types.ts); se valida en tiempo de ejecución.
  interface PromptDoneEvent {
    type: "promptDone";
    id: number;
  }
  function asPromptDone(e: unknown): PromptDoneEvent | null {
    const o = e as { type?: unknown; id?: unknown } | null;
    return typeof o === "object" && o !== null && o.type === "promptDone" && typeof o.id === "number"
      ? { type: "promptDone", id: o.id }
      : null;
  }

  const mac = isMac();

  function run(action: ShortcutAction) {
    const tab = tabById(app.activeTab);
    if (action.startsWith("view:")) app.view = action.slice(5) as typeof app.view;
    else if (action === "newTab") app.quick++;
    else if (action === "closeTab") {
      if (app.view === "terminal" && tab) void requestCloseTab(tab.id);
    } else if (action === "nextTab") cycleTab(1);
    else if (action === "prevTab") cycleTab(-1);
    else if (action === "splitRight" || action === "splitDown") {
      if (tab) splitPane(tab.id, tab.activeLeaf, action === "splitRight" ? "row" : "col");
    } else if (action === "nextPane") focusNextPane();
    else if (action === "find") {
      if (tab && app.view === "terminal") app.find = { leaf: tab.activeLeaf, n: app.find.n + 1 };
    } else if (action === "zoomReset") app.zoom = 0;
    else if (action === "zoomIn" || action === "zoomOut") {
      const base = app.settings.fontSize;
      app.zoom = Math.min(FONT_MAX - base, Math.max(FONT_MIN - base, app.zoom + (action === "zoomIn" ? 1 : -1)));
    }
  }

  // En captura: se resuelve antes de que xterm vea la tecla.
  function onKeydown(e: KeyboardEvent) {
    if (app.confirm || app.prompts.length) return;
    const action = matchShortcut(e, mac);
    if (!action) return;
    e.preventDefault();
    e.stopPropagation();
    run(action);
  }

  onMount(() => {
    void loadSettings();
    // Fuera de Tauri (vite dev en navegador) no hay IPC: se ignora.
    appSubscribe((e) => {
      const done = asPromptDone(e);
      if (done) {
        // Timeout o conexión cancelada: el backend ya no espera respuesta.
        app.prompts = app.prompts.filter((p) => p.id !== done.id);
      } else if (e.type === "prompt") app.prompts.push({ id: e.id, prompt: e.prompt });
      else handleAppEvent(e);
    }).catch(() => {});
  });
</script>

<svelte:window onkeydowncapture={onKeydown} />

<div class="shell">
  <NavRail />
  <main class:bare={app.view === "terminal"}>
    {#if app.view === "hosts"}<HostsView />
    {:else if app.view === "sftp"}<SftpView />
    {:else if app.view === "keychain"}<KeychainView />
    {:else if app.view === "snippets"}<SnippetsView />
    {:else if app.view === "forwards"}<ForwardsView />
    {:else if app.view === "settings"}<SettingsView />{/if}
    <!-- Siempre montado: cambiar de vista no cierra las sesiones. -->
    <div class="ws-host" hidden={app.view !== "terminal"}><Workspace /></div>
  </main>
  <PromptModal />
  <ConfirmModal />
  <div class="toasts" aria-live="polite">
    {#each app.toasts as t (t.id)}
      <div class="toast {t.kind}" role={t.kind === "error" ? "alert" : "status"}>{t.text}</div>
    {/each}
  </div>
</div>

<style>
  .shell { display: flex; height: 100%; }
  main { flex: 1; min-width: 0; overflow: auto; padding: 16px; display: flex; flex-direction: column; }
  main.bare { padding: 0; overflow: hidden; }
  .ws-host { flex: 1; min-height: 0; display: flex; flex-direction: column; }
  .toasts { position: fixed; right: 16px; bottom: 16px; display: grid; gap: 8px; z-index: 60; max-width: min(360px, calc(100vw - 32px)); }
  .toast { background: var(--bg-elev); border: 1px solid var(--border); border-left: 3px solid var(--accent); border-radius: 6px; padding: 10px 12px; box-shadow: 0 4px 16px rgb(0 0 0 / 0.2); }
  .toast.error { border-left-color: var(--danger); }
</style>
