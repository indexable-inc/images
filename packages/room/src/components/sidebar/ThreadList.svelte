<script lang="ts">
  // One section of the sidebar: section label + a list of ThreadRow
  // children. Each row's cursor index is `cursorOffset + i` so the
  // parent's flat cursor can address rows across both the active and
  // archived sections.

  import type { ServerThread } from '$lib/store';
  import ThreadRow from './ThreadRow.svelte';

  interface Props {
    label: string;
    threads: ServerThread[];
    cursorOffset: number;
    activeCursor: number | null;
    routedKey: string | null;
    onOpen: (thread: ServerThread, e: MouseEvent) => void;
    /** Adds the inline label spacing so the section reads as its own
     * block when stacked below another list. The outer grid handles
     * the first section's label. */
    inlineLabel?: boolean;
    /** Per-thread heat in [0, 1] keyed by thread id. The parent owns
     * the scaling (ECDF rank, gamma) so the list view stays a thin
     * lookup. Missing keys render without heatmap styling. */
    heatById?: Map<string, number> | null;
  }

  let {
    label,
    threads,
    cursorOffset,
    activeCursor,
    routedKey,
    onOpen,
    inlineLabel = false,
    heatById = null
  }: Props = $props();
</script>

{#if threads.length > 0}
  {#if inlineLabel}
    <div class="section-label">{label}</div>
  {/if}
  <ul class="list">
    {#each threads as t, i (t.server_id + ':' + t.id)}
      <ThreadRow
        thread={t}
        cursorIndex={cursorOffset + i}
        isCursor={activeCursor !== null && activeCursor === cursorOffset + i}
        isRouted={t.server_id + ':' + t.id === routedKey}
        heat={heatById?.get(t.server_id + ':' + t.id) ?? null}
        {onOpen}
      />
    {/each}
  </ul>
{/if}

<style>
  .section-label {
    padding: 16px 8px 4px;
    color: var(--text-dim);
    font-size: 12px;
    font-weight: 400;
  }
  .list {
    list-style: none;
    margin: 0;
    padding: 0;
  }
</style>
