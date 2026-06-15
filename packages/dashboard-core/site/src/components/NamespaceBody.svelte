<script lang="ts">
  // The namespace renderer: a `data` pane whose body is a JSON array of variable
  // rows produced by the kernel (`introspect.namespace_rows`) — one Python
  // session's live globals. Rows are bucketed into Data / Modules / Functions and
  // rendered as an expandable tree (containers drill into their members), heaviest
  // first within each group, so the eye lands on what holds the memory.
  import type { Pane } from '$lib/types';
  import { groupOf, NS_GROUPS, parseRows, type NsGroup, type NsRow as Row } from '$lib/namespace';
  import NsRow from './NsRow.svelte';

  let { pane }: { pane: Pane } = $props();

  const rows = $derived(parseRows(pane.body));

  // Group preserving the producer's heaviest-first order within each bucket.
  const groups = $derived.by<{ name: NsGroup; rows: Row[] }[]>(() => {
    const by: Record<NsGroup, Row[]> = { Data: [], Modules: [], Functions: [] };
    for (const row of rows) by[groupOf(row)].push(row);
    return NS_GROUPS.map((name) => ({ name, rows: by[name] })).filter((g) => g.rows.length > 0);
  });
</script>

<div class="ns">
  {#if rows.length === 0}
    <div class="ns-empty">no variables</div>
  {:else}
    {#each groups as group (group.name)}
      <div class="ns-group">
        <div class="ns-grouphead">{group.name}<span class="ns-groupn">{group.rows.length}</span></div>
        {#each group.rows as row, i (row.name + ':' + i)}
          <NsRow {row} />
        {/each}
      </div>
    {/each}
  {/if}
</div>

<style>
  .ns {
    padding: 4px 0 8px;
  }
  .ns-empty {
    padding: 10px 14px;
    color: var(--ink-faint);
    font-family: var(--mono);
    font-size: 12px;
    font-style: italic;
  }
  .ns-group + .ns-group {
    margin-top: 6px;
  }
  /* A quiet section label, the same uppercase micro-heading the rest of the app
     uses for groups. */
  .ns-grouphead {
    display: flex;
    align-items: baseline;
    gap: 8px;
    padding: 10px 14px 6px;
    font-size: 10px;
    font-weight: 600;
    letter-spacing: 0.12em;
    text-transform: uppercase;
    color: var(--ink-faint);
  }
  .ns-groupn {
    color: var(--ink-faint);
    opacity: 0.6;
    font-variant-numeric: tabular-nums;
  }
</style>
