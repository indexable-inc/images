<script lang="ts">
  // The right rail: the namespace inspector for the selected run's session. It
  // finds the namespace pane sharing the run's scope and renders it as an
  // expandable tree (reusing NamespaceBody/NsRow/KindIcon). Collapsible via a thin
  // toggle strip; the collapsed state is persisted in ui. Expansion is mouse-driven
  // here — the sidebar owns the keyboard.
  import { store } from '$lib/stream.svelte';
  import { ui, toggleRail } from '$lib/ui.svelte';
  import { paneScope } from '$lib/scope';
  import { buildNsItems, parseRows } from '$lib/namespace';
  import { withKey, isNamespacePane } from '$lib/run';
  import NamespaceBody from './NamespaceBody.svelte';
  import type { Pane } from '$lib/types';

  // The scope whose namespace to show: the selected run/resource's scope.
  let { scope }: { scope: string } = $props();

  // The namespace pane for this scope, if the session publishes one.
  const nsPane = $derived.by<Pane | null>(() => {
    for (const key of Object.keys(store.panes)) {
      const rec = store.panes[key];
      if (paneScope(key) === scope && isNamespacePane(rec)) return withKey(key, rec, scope);
    }
    return null;
  });

  // Local expansion state for the tree (keyed by row path); reset when the pane
  // changes so a new session starts collapsed.
  let expanded = $state<Record<string, boolean>>({});
  const items = $derived(nsPane ? buildNsItems(parseRows(nsPane.body), expanded, 'rail') : []);
  const count = $derived(items.filter((it) => it.kind === 'row' && it.depth === 0).length);

  function onToggle(path: string): void {
    expanded[path] = !expanded[path];
  }
</script>

<div class="rail-wrap">
  <button
    class="rail-toggle"
    class:collapsed={ui.railCollapsed}
    aria-label={ui.railCollapsed ? 'show namespace' : 'hide namespace'}
    title={ui.railCollapsed ? 'show namespace' : 'hide namespace'}
    onclick={toggleRail}
  >{ui.railCollapsed ? '‹' : '›'}</button>

  {#if !ui.railCollapsed}
    <aside class="rail">
      <div class="rail-head">
        <span class="rail-label">namespace</span>
        {#if nsPane}<span class="rail-count">{count}</span>{/if}
      </div>
      {#if nsPane}
        <div class="rail-body">
          <NamespaceBody pane={nsPane} {items} {expanded} onToggle={onToggle} />
        </div>
      {:else}
        <div class="rail-empty">no namespace for this session</div>
      {/if}
    </aside>
  {/if}
</div>

<style>
  .rail-wrap {
    flex: none;
    display: flex;
    min-height: 0;
  }
  /* A thin always-present strip that folds the rail. */
  .rail-toggle {
    width: 14px;
    flex: none;
    background: var(--panel);
    border: 0;
    border-left: 1px solid var(--edge);
    cursor: pointer;
    color: var(--ink-faint);
    font-family: var(--mono);
    font-size: 11px;
    display: flex;
    align-items: center;
    justify-content: center;
  }
  .rail-toggle:hover {
    color: var(--ink-dim);
    background: var(--elev, var(--panel));
  }
  .rail {
    width: 240px;
    flex: none;
    background: var(--panel);
    border-left: 1px solid var(--edge);
    display: flex;
    flex-direction: column;
    min-height: 0;
  }
  .rail-head {
    flex: none;
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 10px 12px 8px;
    border-bottom: 1px solid var(--edge);
  }
  .rail-label {
    font-family: var(--mono);
    font-size: 11px;
    letter-spacing: 0.06em;
    text-transform: uppercase;
    color: var(--ink-dim);
  }
  .rail-count {
    margin-left: auto;
    font-family: var(--mono);
    font-size: 10px;
    color: var(--ink-faint);
    background: var(--elev, var(--panel));
    border: 1px solid var(--edge);
    padding: 1px 6px;
    font-variant-numeric: tabular-nums;
  }
  .rail-body {
    flex: 1 1 auto;
    min-height: 0;
    overflow-y: auto;
  }
  .rail-empty {
    padding: 14px 12px;
    color: var(--ink-faint);
    font-family: var(--mono);
    font-size: 11.5px;
    font-style: italic;
  }
</style>
