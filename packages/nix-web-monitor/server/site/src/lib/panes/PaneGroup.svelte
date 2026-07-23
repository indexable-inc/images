<script lang="ts">
  /// A tab group: one dock region shared by a stack of panes, one visible at a
  /// time. The tab bar doubles as the pane header (the active pane's
  /// `controls` snippet renders on its right) and as the drag-and-drop surface
  /// for moving tabs between groups.

  import { getDockContext } from '$lib/panes/context';
  import PaneVisibility from '$lib/panes/PaneVisibility.svelte';
  import type { GroupNode, PaneId, SplitDirection } from '$lib/panes/types';

  type Props = {
    group: GroupNode;
    /// Axis of the parent split, when any: a group collapsed inside a `row`
    /// split shrinks to a vertical strip (sideways tabs) instead of a bar.
    parentDirection?: SplitDirection;
  };

  const { group, parentDirection }: Props = $props();
  const dock = getDockContext();

  let element = $state<HTMLElement | null>(null);
  let dropTarget = $state(false);

  const visibleTabs = $derived(group.tabs.filter((id) => !dock.hidden(id)));
  /// The tab whose content shows. Falls back off a hidden active tab without
  /// mutating state, so the choice comes back when the pane does.
  const activeId = $derived(
    group.active !== null && visibleTabs.includes(group.active)
      ? group.active
      : (visibleTabs.at(0) ?? null)
  );
  const activeSpec = $derived(activeId === null ? undefined : dock.spec(activeId));
  const sideways = $derived(group.collapsed && parentDirection === 'row');

  function title(id: PaneId): string {
    return dock.spec(id)?.title ?? id;
  }

  function onTabDragStart(event: DragEvent, id: PaneId): void {
    dock.state.dragging = id;
    if (event.dataTransfer !== null) {
      event.dataTransfer.setData('text/plain', id);
      event.dataTransfer.effectAllowed = 'move';
    }
  }

  function onTabDragEnd(): void {
    dock.state.dragging = null;
    dropTarget = false;
  }

  function onBarDragOver(event: DragEvent): void {
    if (dock.state.dragging === null) return;
    event.preventDefault();
    if (event.dataTransfer !== null) event.dataTransfer.dropEffect = 'move';
    dropTarget = true;
  }

  function onBarDragLeave(): void {
    dropTarget = false;
  }

  /// Drop on the bar's empty space appends; drop on a tab inserts before it
  /// (the tab handler stops propagation so both can share the bar).
  function onBarDrop(event: DragEvent): void {
    event.preventDefault();
    dropTarget = false;
    moveDragged(group.tabs.length, event);
  }

  function onTabDrop(event: DragEvent, id: PaneId): void {
    event.preventDefault();
    event.stopPropagation();
    dropTarget = false;
    moveDragged(group.tabs.indexOf(id), event);
  }

  function moveDragged(index: number, event: DragEvent): void {
    const id = dock.state.dragging ?? event.dataTransfer?.getData('text/plain') ?? '';
    dock.state.dragging = null;
    if (id.length === 0) return;
    dock.state.moveTab(id, group, Math.max(0, index));
  }

  /// Promote the active pane to a floating window seeded over this group's
  /// current footprint (shrunk and nudged so the dock shows behind it).
  function popOut(): void {
    if (activeId === null) return;
    const dockRect = dock.dockElement()?.getBoundingClientRect();
    const rect = element?.getBoundingClientRect();
    if (dockRect === undefined || rect === undefined) return;
    dock.state.popOut(activeId, {
      x: rect.left - dockRect.left + 24,
      y: rect.top - dockRect.top + 24,
      width: Math.max(280, Math.min(rect.width - 48, dockRect.width - 96)),
      height: Math.max(180, Math.min(rect.height - 48, dockRect.height - 96))
    });
  }
</script>

<section class="pane-group" class:collapsed={group.collapsed} class:sideways bind:this={element}>
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <header
    class="pane-tabs"
    class:drop={dropTarget}
    ondragover={onBarDragOver}
    ondragleave={onBarDragLeave}
    ondrop={onBarDrop}
  >
    <div class="pane-tab-list" role="tablist">
      {#each visibleTabs as id (id)}
        <button
          type="button"
          class="pane-tab"
          class:active={id === activeId && !group.collapsed}
          role="tab"
          aria-selected={id === activeId}
          draggable="true"
          ondragstart={(event) => {
            onTabDragStart(event, id);
          }}
          ondragend={onTabDragEnd}
          ondrop={(event) => {
            onTabDrop(event, id);
          }}
          onclick={() => {
            dock.state.activate(group, id);
          }}
        >
          {title(id)}
        </button>
      {/each}
    </div>
    <div class="pane-actions">
      {#if !group.collapsed && activeSpec?.controls !== undefined}
        <div class="pane-controls">{@render activeSpec.controls()}</div>
      {/if}
      <button
        type="button"
        class="pane-action"
        title="pop out"
        aria-label="pop out {activeId === null ? 'pane' : title(activeId)}"
        onclick={popOut}
      >
        ⧉
      </button>
      <button
        type="button"
        class="pane-action"
        title={group.collapsed ? 'expand' : 'collapse'}
        aria-expanded={!group.collapsed}
        onclick={() => {
          dock.state.toggleCollapsed(group);
        }}
      >
        {group.collapsed ? (sideways ? '◂' : '▸') : '▾'}
      </button>
    </div>
  </header>
  <!-- Every visible tab's pane stays mounted; switching tabs or collapsing
       the group only toggles CSS visibility. Pane-local operator state (the
       log pane's level/search filters, the build table's tree/flat choice and
       collapsed nodes) survives exactly as it did in the old fixed layout,
       which kept every panel mounted for the whole session -- and so does
       that layout's render cost: a hidden pane's effects (e.g. an expanded
       machine-build log drawer's poll) keep running, as they always did.
       Global *input*, though, must not: a mounted-but-hidden pane's
       window-level shortcuts would steal keystrokes from the visible pane, so
       each slot bridges its shown/hidden state into the content via
       PaneVisibility for the content's handlers to gate on.
       Keyed by pane id like the tab bar; a tab dragged to *another* group
       still remounts there, since it moves between two keyed lists. -->
  {#each visibleTabs as id (id)}
    {@const spec = dock.spec(id)}
    {#if spec !== undefined}
      <div class="pane-content" hidden={group.collapsed || id !== activeId}>
        <PaneVisibility visible={!group.collapsed && id === activeId}>
          {@render spec.content()}
        </PaneVisibility>
      </div>
    {/if}
  {/each}
</section>

<style>
  .pane-group {
    display: flex;
    flex-direction: column;
    min-width: 0;
    min-height: 0;
    background: var(--panel, #ffffff);
    overflow: hidden;
  }

  .pane-tabs {
    flex: 0 0 auto;
    display: flex;
    align-items: stretch;
    gap: 0.5rem;
    border-bottom: 1px solid var(--line, #d4d4d8);
    background: var(--panel-soft, #f1f5f9);
    min-width: 0;
  }

  .pane-tabs.drop {
    outline: 1px solid var(--accent, #2563eb);
    outline-offset: -1px;
  }

  .pane-tab-list {
    display: flex;
    align-items: stretch;
    min-width: 0;
    overflow-x: auto;
    scrollbar-width: none;
  }

  .pane-tab {
    appearance: none;
    border: 0;
    border-right: 1px solid var(--line-soft, #e4e4e7);
    background: transparent;
    color: var(--muted, #6b7280);
    font-family: inherit;
    padding: 0.4rem 0.75rem;
    cursor: pointer;
    text-transform: uppercase;
    font-weight: 700;
    letter-spacing: 0.05em;
    white-space: nowrap;
  }

  .pane-tab:hover {
    color: var(--ink, #111827);
  }

  .pane-tab.active {
    background: var(--panel, #ffffff);
    color: var(--ink, #111827);
    box-shadow: inset 0 2px 0 var(--accent, #2563eb);
  }

  .pane-actions {
    margin-left: auto;
    display: flex;
    align-items: center;
    gap: 0.4rem;
    padding: 0 0.4rem;
    min-width: 0;
  }

  .pane-controls {
    display: flex;
    align-items: center;
    gap: 0.4rem;
    min-width: 0;
  }

  .pane-action {
    appearance: none;
    border: 0;
    background: transparent;
    color: var(--faint, #9ca3af);
    font-family: inherit;
    cursor: pointer;
    padding: 0 0.15rem;
    line-height: 1;
  }

  .pane-action:hover {
    color: var(--ink, #111827);
  }

  .pane-content {
    flex: 1 1 auto;
    min-height: 0;
    min-width: 0;
    display: flex;
    flex-direction: column;
    overflow: hidden;
  }

  /* An inactive tab's pane stays in the DOM (state survives tab switches and
   * collapse) but must not paint or take layout. Explicit because the
   * author-level `display: flex` above would otherwise override the UA
   * stylesheet's `[hidden] { display: none }`. */
  .pane-content[hidden] {
    display: none;
  }

  /* Collapsed inside a row split: the group narrows to a vertical strip and
   * the tabs read sideways, ghostty/vscode-sidebar style. */
  .pane-group.sideways {
    flex-direction: row;
  }

  .pane-group.sideways .pane-tabs {
    flex-direction: column;
    writing-mode: vertical-rl;
    border-bottom: 0;
    border-left: 1px solid var(--line, #d4d4d8);
  }

  .pane-group.sideways .pane-tab-list {
    overflow: hidden;
  }

  .pane-group.sideways .pane-actions {
    margin-left: 0;
    margin-top: auto;
    padding: 0.4rem 0;
  }
</style>
