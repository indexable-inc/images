<script lang="ts">
  // The namespace renderer for one Python session: a `data` pane whose body is a
  // JSON array of variable rows (`introspect.namespace_rows`). Rows are bucketed
  // into Data / Modules / Functions and rendered as an expandable tree, heaviest
  // first within each group. Selection and expansion are owned by the parent view
  // (so the keyboard can walk the whole tree across sessions); this component just
  // renders the flattened item list the shared builder produces.
  import type { Pane } from '$lib/types';
  import { buildNsItems, parseRows } from '$lib/namespace';
  import NsRow from './NsRow.svelte';

  // Controlled by the Namespace view (selection + expansion lifted out for
  // keyboard nav), or uncontrolled when used as the generic `namespace` renderer
  // — in which case it keeps its own expand state and ignores selection.
  let {
    pane,
    prefix = 'r',
    expanded,
    selected = null,
    onSelect,
    onToggle,
  }: {
    pane: Pane;
    prefix?: string;
    expanded?: Record<string, boolean>;
    selected?: string | null;
    onSelect?: (path: string) => void;
    onToggle?: (path: string) => void;
  } = $props();

  // Fallback expand state for uncontrolled use.
  let localExpanded = $state<Record<string, boolean>>({});
  const exp = $derived(expanded ?? localExpanded);
  const select = $derived(onSelect ?? (() => {}));
  const toggle = $derived(
    onToggle ??
      ((path: string) => {
        localExpanded[path] = !localExpanded[path];
      }),
  );

  const items = $derived(buildNsItems(parseRows(pane.body), exp, prefix));
</script>

<div class="ns">
  {#if items.length === 0}
    <div class="ns-empty">no variables</div>
  {:else}
    {#each items as it (it.kind === 'group' ? 'g:' + it.name : it.path)}
      {#if it.kind === 'group'}
        <div class="ns-grouphead">{it.name}<span class="ns-groupn">{it.count}</span></div>
      {:else}
        <NsRow
          row={it.row}
          depth={it.depth}
          path={it.path}
          scope={pane.scope}
          open={!!exp[it.path]}
          selected={selected === it.path}
          onSelect={select}
          onToggle={toggle}
        />
      {/if}
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
  .ns-grouphead:not(:first-child) {
    margin-top: 6px;
  }
  .ns-groupn {
    color: var(--ink-faint);
    opacity: 0.6;
    font-variant-numeric: tabular-nums;
  }
</style>
