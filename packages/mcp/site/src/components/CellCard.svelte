<script lang="ts">
  import type { Cell } from '$lib/types';
  import RichOutput from './RichOutput.svelte';

  // One curated presentation cell: the agent's chosen title over its rendered
  // outputs (a table, an image, a Result's HTML). This is the highlight reel,
  // so it leads with the title and shows the output large and unadorned.
  let { cell }: { cell: Cell } = $props();
</script>

<article class="cell">
  {#if cell.title}<header class="cell-hdr">{cell.title}</header>{/if}
  {#each cell.outputs as output, i (i)}
    <RichOutput {output} />
  {/each}
</article>

<style>
  .cell {
    margin: 0 0 12px;
    padding: 12px 14px;
    background: var(--panel);
    border: 1px solid var(--line);
    border-left: 2px solid var(--accent, #6ea8fe);
  }
  .cell-hdr {
    margin: 0 0 4px;
    color: var(--text);
    font-size: 13px;
    font-weight: 600;
    letter-spacing: 0.01em;
  }
</style>
