<script lang="ts">
  import type { FolderNode } from "../lib/folderTree";
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
        style:padding-left="{8 + depth * 14}px"
        onclick={() => onselect(n.path)}
      >
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
  ul { list-style: none; margin: 0; padding: 0; }
  .folder { width: 100%; display: flex; justify-content: space-between; align-items: center; gap: 6px; border: 0; border-radius: 6px; background: none; padding: 5px 8px; text-align: left; color: var(--fg); }
  .folder.on { background: var(--bg-hover); color: var(--accent); }
  .name { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .count { color: var(--fg-dim); font-size: 12px; }
</style>
