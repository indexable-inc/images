<script lang="ts">
  // The edit history: every change in the shared document, newest first.
  //
  // One row is one CRDT change. It says WHO made it (the `__peers` table names
  // agents and humans; an unregistered peer still gets an identity built from its
  // peer id, so an edit is never attributed to nobody), WHAT it did (its ops,
  // reduced to a line — a set value, a pane appearing, the size of a text edit;
  // never a position, because a JSON op's `pos` is an entity index and not a
  // character offset), and WHEN, as a bare relative age.
  //
  // Clicking a row selects the pane the edit landed in and marks the exact field
  // in the document. The mark is DRAWN — an accent fill and an accent rule — and
  // stays until another row is clicked. Nothing here is dimmed and nothing
  // animates: the scroll is instant and a row change repaints, it does not fade.
  import { store, edits, markEdit } from '$lib/stream.svelte';
  import { ui, select, toggleHistory, shortAge } from '$lib/ui.svelte';
  import { peerBadge } from '$lib/peers';
  import { isResource } from '$lib/run';
  import type { EditRow } from '$lib/edits';

  // Newest first. `edits.rows` is oldest-first (the oplog's own order), so this is
  // the one place that flips it.
  const rows = $derived([...edits.rows].reverse());

  // The pane on the centre stage, so rows that touch what is on screen can say so
  // without the user clicking each one.
  const onStage = $derived(
    ui.selection && ui.selection.kind !== 'recording' ? ui.selection.key : '',
  );

  function open(row: EditRow): void {
    // Clicking the marked row again clears the mark rather than re-scrolling.
    if (edits.marked === row.id) {
      markEdit(null);
      return;
    }
    markEdit(row.id);
    const target = row.target;
    if (!target) return;
    const record = store.panes[target.paneKey];
    // A pane that has since been deleted keeps its row but has nowhere to scroll.
    if (!record) return;
    select({
      kind: isResource(target.paneKey, record) ? 'resource' : 'run',
      key: target.paneKey,
    });
  }
</script>

<aside class="hist">
  <div class="hist-head">
    <span class="hist-label">edits</span>
    <span class="hist-count">{edits.rows.length}</span>
    <button class="hist-close" aria-label="hide edit history" onclick={toggleHistory}>×</button>
  </div>

  {#if rows.length}
    <div class="hist-rows">
      {#each rows as row (row.id)}
        {@const who = peerBadge(store.peers, row.peer, edits.localPeer)}
        <button
          class="hist-row"
          class:marked={edits.marked === row.id}
          class:here={!!row.target && row.target.paneKey === onStage}
          onclick={() => open(row)}
          title={row.where ? `${row.what} — ${row.where}` : row.what}
        >
          <span class="hist-top">
            <span class="hist-who" class:agent={who.kind === 'agent'} class:human={who.kind === 'human'} class:you={who.you}
              >{who.label}</span
            >
            <span class="hist-what">{row.what}</span>
            <span class="hist-when">{shortAge(row.ts, ui.clock)}</span>
          </span>
          {#if row.where}<span class="hist-where">{row.where}</span>{/if}
        </button>
      {/each}
    </div>
  {:else}
    <div class="hist-empty">no edits yet</div>
  {/if}
</aside>

<style>
  .hist {
    width: 268px;
    flex: none;
    background: var(--panel);
    border-left: 1px solid var(--edge);
    display: flex;
    flex-direction: column;
    min-height: 0;
  }
  .hist-head {
    flex: none;
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 10px 8px 8px 12px;
    border-bottom: 1px solid var(--edge);
  }
  .hist-label {
    font-family: var(--mono);
    font-size: 11px;
    letter-spacing: 0.06em;
    text-transform: uppercase;
    color: var(--ink-dim);
  }
  .hist-count {
    margin-left: auto;
    font-family: var(--mono);
    font-size: 10px;
    color: var(--ink-faint);
    background: var(--elev, var(--panel));
    border: 1px solid var(--edge);
    padding: 1px 6px;
    font-variant-numeric: tabular-nums;
  }
  .hist-close {
    font-family: var(--mono);
    font-size: 12px;
    line-height: 1;
    color: var(--ink-faint);
    background: none;
    border: 0;
    padding: 2px 4px;
    cursor: pointer;
  }
  .hist-close:hover {
    color: var(--ink);
  }

  .hist-rows {
    flex: 1 1 auto;
    min-height: 0;
    overflow-y: auto;
  }
  /* One dense row: who, what, when. Square, hairline-separated, flat. */
  .hist-row {
    display: block;
    width: 100%;
    text-align: left;
    font: inherit;
    background: none;
    border: 0;
    border-bottom: 1px solid var(--edge);
    /* The 2px gutter is where the "this is on screen" and "this is marked" rules
       are drawn, so a row never shifts when either turns on. */
    border-left: 2px solid transparent;
    padding: 5px 10px 5px 8px;
    cursor: pointer;
  }
  .hist-row:hover {
    background: var(--sel);
  }
  /* On screen right now: a quiet accent tick in the gutter. */
  .hist-row.here {
    border-left-color: color-mix(in srgb, var(--accent) 45%, transparent);
  }
  /* The row being looked at: drawn with the accent, never by dimming the rest. */
  .hist-row.marked {
    background: var(--accent-soft);
    border-left-color: var(--accent);
  }
  .hist-top {
    display: flex;
    align-items: baseline;
    gap: 7px;
    min-width: 0;
  }
  /* Attribution reads as ink, not as a badge: agents blue, this browser accent,
     an unregistered peer the muted rank. Nothing is faded to say "less sure". */
  .hist-who {
    flex: none;
    max-width: 78px;
    font-family: var(--mono);
    font-size: 10.5px;
    color: var(--ink-dim);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .hist-who.agent {
    color: var(--k-blue);
  }
  .hist-who.human {
    color: var(--ink);
  }
  .hist-who.you {
    color: var(--accent);
  }
  .hist-what {
    flex: 1 1 auto;
    min-width: 0;
    font-family: var(--mono);
    font-size: 11.5px;
    color: var(--ink);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .hist-when {
    flex: none;
    font-family: var(--mono);
    font-size: 10.5px;
    color: var(--ink-faint);
    font-variant-numeric: tabular-nums;
    min-width: 3ch;
    text-align: right;
  }
  /* Where it landed, so a row says its location without being clicked. */
  .hist-where {
    display: block;
    margin-top: 1px;
    padding-left: 85px;
    font-family: var(--mono);
    font-size: 10px;
    color: var(--ink-faint);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .hist-empty {
    padding: 14px 12px;
    color: var(--ink-faint);
    font-family: var(--mono);
    font-size: 11.5px;
    font-style: italic;
  }
</style>
