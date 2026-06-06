<script lang="ts">
  // One thread row in the sidebar list. Renders the title and the
  // right-hand affordances: viewer stack + timestamp for live chats,
  // a "Draft" pill for unsent local drafts, and a quiet timestamp for
  // archived chats. The parent owns the cursor and the routed
  // selection; this component just paints them.

  import type { ServerThread } from '$lib/store';
  import { DRAFT_STATUS } from '$lib/drafts';
  import { relativeTimeShort } from '$lib/time';
  import { nowTick } from '$lib/activity';
  import { agentWorkMode } from '$lib/agentWork';
  import ViewerStack from '$components/ViewerStack.svelte';
  import WorkGlyph from '$components/WorkGlyph.svelte';

  interface Props {
    thread: ServerThread;
    cursorIndex: number;
    isCursor: boolean;
    isRouted: boolean;
    /** Normalized 0..1 activity heat. Null disables heatmap styling
     * (e.g. archived section or single-row lists). */
    heat?: number | null;
    onOpen: (thread: ServerThread, e: MouseEvent) => void;
  }

  let {
    thread,
    cursorIndex,
    isCursor,
    isRouted,
    heat = null,
    onOpen
  }: Props = $props();

  let isDraft = $derived(thread.status === DRAFT_STATUS);
  let isArchived = $derived(thread.status === 'archived');
  // Work-state indicator on the left edge. The server tracks status
  // on every hook event: 'active' while Codex is working, 'blocked'
  // when a PermissionRequest is open (user needs to act), 'idle'
  // after Stop. Drafts and archived threads have no worker to
  // indicate. agentWorkMode() also gates on recency so a thread whose
  // codex turn died mid-flight stops spinning after the quiet
  // window — without that the column would advertise dead workers.
  let nowMs = $state(Date.now());
  $effect(() => nowTick.subscribe((v) => (nowMs = v)));
  let workState = $derived(
    isDraft || isArchived ? null : agentWorkMode(thread, nowMs)
  );

  let rowStyle = $derived(heat == null ? '' : `--heat: ${heat.toFixed(3)};`);
</script>

<li>
  <a
    class="row"
    class:active={isRouted}
    class:cursor={isCursor}
    class:draft={isDraft}
    class:archived={isArchived}
    class:heat={heat != null}
    class:has-work={workState !== null}
    data-cursor-index={cursorIndex}
    style={rowStyle}
    href={`room://s/${encodeURIComponent(thread.server_id)}/t/${encodeURIComponent(thread.id)}`}
    onclick={(e) => onOpen(thread, e)}
  >
    {#if workState !== null}
      <span class="work-slot">
        <WorkGlyph mode={workState} />
      </span>
    {/if}
    <span class="row-title" title={thread.title || 'Untitled'}>
      {thread.title || 'Untitled'}
    </span>
    <span class="row-right">
      {#if isDraft}
        <span class="draft-badge">Draft</span>
      {:else if isArchived}
        <span class="row-time">{relativeTimeShort(thread.updated_ms)}</span>
      {:else}
        <ViewerStack serverId={thread.server_id} threadId={thread.id} size={16} max={3} />
        <span class="row-time">{relativeTimeShort(thread.updated_ms)}</span>
      {/if}
    </span>
  </a>
</li>

<style>
  .row {
    position: relative;
    display: flex;
    align-items: center;
    gap: 8px;
    min-height: 30px;
    padding: 5px 10px;
    margin: 1px 0;
    border-radius: var(--radius-sm);
    color: var(--text);
    font-size: 13px;
    min-width: 0;
    line-height: 1.4;
    transition: background 0.08s ease;
  }
  /* Make room for the left-edge work indicator on rows that have one.
     The bar sits inside the row's padding so this is just a small
     extra inset to keep the title from kissing it. */
  .row.has-work {
    padding-left: 14px;
  }
  .row:hover {
    background: var(--bg-hover);
    color: var(--text-strong);
  }
  .row.active {
    background: var(--bg-selected);
    color: var(--text-strong);
    font-weight: 500;
  }
  /* Unsent draft thread: italic title + small dim "Draft" pill in
     place of the viewer stack / timestamp. */
  .row.draft .row-title {
    font-style: italic;
    color: var(--text-muted);
  }
  .row.draft.active .row-title {
    color: var(--text-strong);
  }
  /* Archived rows read as a quiet second tier: muted title until the
     row is hovered, focused by the cursor, or the routed selection. */
  .row.archived .row-title {
    color: var(--text-muted);
  }
  .row.archived:hover .row-title,
  .row.archived.active .row-title,
  .row.archived.cursor .row-title {
    color: var(--text-strong);
  }
  .draft-badge {
    color: var(--text-dim);
    font-size: 10.5px;
    letter-spacing: 0.02em;
    text-transform: uppercase;
    padding: 1px 6px;
    border-radius: 999px;
    background: var(--bg-pill);
  }
  .row-title {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .row-right {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    flex-shrink: 0;
    color: var(--text-dim);
    font-size: 11.5px;
  }
  .row-time {
    color: var(--text-dim);
    font-size: 11.5px;
    font-variant-numeric: tabular-nums;
  }
  /* Keyboard focus cursor (j/k). IntelliJ-style: a flat selection
     fill — no left accent rule, no side bar. Darker than the
     unfocused/routed highlights so the eye lands on it immediately
     while the sidebar owns the keyboard. Derived from text-strong
     so it scales correctly under both light and dark themes. */
  .row.cursor {
    background: color-mix(in srgb, var(--text-strong) 14%, transparent);
    color: var(--text-strong);
  }
  /* While the sidebar owns the keyboard, dim the routed highlight a
     touch so the cursor reads as the primary selection. Selector lives
     on the parent .sidebar so it only fires inside the focused panel. */
  :global(.sidebar.active) .row.active:not(.cursor) {
    background: transparent;
    color: var(--text);
    font-weight: 400;
  }

  /* Activity heatmap. The parent ThreadList computes a 0..1 value per
     row from sqrt(message_count) / max and exposes it as --heat on the
     row element. Cold rows fade toward --text-dim; hot rows reach
     --text-strong and pick up a faint background tint so the column
     reads as a heat strip at a glance. Hover, cursor, and routed
     states still override with full strength below. */
  .row.heat .row-title {
    color: color-mix(
      in srgb,
      var(--text-strong) calc(var(--heat) * 100%),
      var(--text-dim)
    );
  }
  .row.heat {
    background: transparent;
  }
  /* Override the heat tints when the row is interactive or selected so
     those states still pop as the primary signal. */
  .row.heat:hover,
  .row.heat.active,
  .row.heat.cursor {
    background: var(--bg-hover);
  }
  .row.heat:hover .row-title,
  .row.heat.active .row-title,
  .row.heat.cursor .row-title {
    color: var(--text-strong);
  }
  .row.heat.active {
    background: var(--bg-selected);
  }
  .row.heat.cursor {
    background: color-mix(in srgb, var(--text-strong) 14%, transparent);
  }

  /* Work-state glyph pinned to the row's left edge. The actual glyph
     is rendered by <WorkGlyph>; this just gives it a fixed 12px slot
     so the column reads as a clean status rail down the left edge of
     the list. */
  .work-slot {
    position: absolute;
    left: 1px;
    top: 0;
    bottom: 0;
    width: 12px;
    display: flex;
    align-items: center;
    justify-content: center;
    pointer-events: none;
  }
</style>
