<script lang="ts">
  import { onMount } from "svelte";
  import { appSubscribe } from "./lib/api";
  import { app } from "./lib/state.svelte";
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
    }).catch(() => {});
  });
</script>

<div class="shell">
  <NavRail />
  <main>
    {#if app.view === "hosts"}<HostsView />
    {:else if app.view === "sftp"}<SftpView />
    {:else if app.view === "keychain"}<KeychainView />
    {:else if app.view === "snippets"}<SnippetsView />
    {:else}<ForwardsView />{/if}
    <Workspace />
  </main>
  <PromptModal />
</div>

<style>
  .shell { display: flex; height: 100%; }
  main { flex: 1; min-width: 0; overflow: auto; padding: 16px; }
</style>
