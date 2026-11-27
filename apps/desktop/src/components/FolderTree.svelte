<script lang="ts">
  import { hueOf, type FolderNode } from "../lib/folderTree";
  import FolderTree from "./FolderTree.svelte";

  let {
    nodes,
    selected,
    depth = 0,
    onselect,
  }: {
    nodes: FolderNode[];
    selected: string | null;
    depth?: number;
    onselect: (path: string) => void;
  } = $props();
</script>

<ul>
  {#each nodes as n (n.path)}
    <li>
      <button
        class="folder"
        class:on={selected === n.path}
        aria-pressed={selected === n.path}
        style:padding-left="{10 + depth * 14}px"
        onclick={() => onselect(n.path)}
      >
        <span class="sw" style:--h={hueOf(n.path.split("/")[0])} aria-hidden="true"></span>
        <span class="name">{n.name}</span>
        <span class="count">{n.count}</span>
      </button>
      {#if n.children.length > 0}
        <FolderTree nodes={n.children} {selected} depth={depth + 1} {onselect} />
      {/if}
    </li>
  {/each}
</ul>

<style>
  ul { list-style: none; margin: 0; padding: 0; display: grid; gap: 1px; }
  .folder { width: 100%; height: 30px; justify-content: flex-start; gap: 9px; border: 0; background: none; padding-right: 10px; font-weight: 400; color: var(--fg); }
  .folder.on, .folder.on:hover:not(:disabled) { background: var(--bg-active); font-weight: 500; }
  .sw { width: 8px; height: 8px; border-radius: 3px; flex: none; background: oklch(var(--av-fg-l) var(--av-fg-c) var(--h)); }
  .name { flex: 1; min-width: 0; text-align: left; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .count { color: var(--fg-faint); font-size: 11.5px; font-variant-numeric: tabular-nums; }
</style>
