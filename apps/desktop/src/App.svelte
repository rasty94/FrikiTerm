<script lang="ts">
  import { onMount } from "svelte";
  import { appSubscribe } from "./lib/api";
  import { app } from "./lib/state.svelte";
  import { handleAppEvent } from "./lib/sessions.svelte";
  import NavRail from "./components/NavRail.svelte";
  import PromptModal from "./components/PromptModal.svelte";
  import HostsView from "./views/HostsView.svelte";
  import Workspace from "./views/Workspace.svelte";
  import SftpView from "./views/SftpView.svelte";
  import KeychainView from "./views/KeychainView.svelte";
  import SnippetsView from "./views/SnippetsView.svelte";
  import ForwardsView from "./views/ForwardsView.svelte";

  onMount(() => {
    // Fuera de Tauri (vite dev en navegador) no hay IPC: se ignora.
    appSubscribe((e) => {
      if (e.type === "prompt") app.prompts.push({ id: e.id, prompt: e.prompt });
      else handleAppEvent(e);
    }).catch(() => {});
  });
</script>

<div class="shell">
  <NavRail />
  <main class:bare={app.view === "terminal"}>
    {#if app.view === "hosts"}<HostsView />
    {:else if app.view === "sftp"}<SftpView />
    {:else if app.view === "keychain"}<KeychainView />
    {:else if app.view === "snippets"}<SnippetsView />
    {:else if app.view === "forwards"}<ForwardsView />{/if}
    <!-- Siempre montado: cambiar de vista no cierra las sesiones. -->
    <div class="ws-host" hidden={app.view !== "terminal"}><Workspace /></div>
  </main>
  <PromptModal />
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
