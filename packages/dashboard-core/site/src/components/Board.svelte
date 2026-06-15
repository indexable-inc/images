<script lang="ts">
  // The board is a tiling window manager for the session's live *resources* — the
  // long-lived, interactive surfaces (TUIs and browsers), not the one-shot run
  // outputs the feed already shows. Like i3's automatic tiling it fills the whole
  // stage with no gaps and no overlap; there is nothing to pan, zoom, or drag.
  // Keyboard nav comes from the global vim keymap (j/k to move, o/Enter to open a
  // resource fullscreen), same as every other view.
  import { onMount } from 'svelte';
  import { store, SCOPE_SEP } from '$lib/stream.svelte';
  import { focusPane } from '$lib/ui.svelte';
  import { setListNav } from '$lib/keys.svelte';
  import { rendererFor } from '$lib/renderers';
  import type { Pane } from '$lib/types';

  // A resource is a TUI (a `terminal` pane) or a live resource the kernel
  // publishes (a `resource/<id>` html pane: browser, vm, …). Everything else —
  // execs, data, namespaces, per-run outputs — belongs to the feed/namespace
  // views, not the board.
  function isResource(key: string, p: Pane): boolean {
    if ((p.kind ?? 'data') === 'terminal') return true;
    const sep = key.indexOf(SCOPE_SEP);
    const id = sep === -1 ? key : key.slice(sep + 1);
    return id.startsWith('resource/');
  }

  const resources = $derived(
    Object.keys(store.panes)
      .map((key) => {
        const sep = key.indexOf(SCOPE_SEP);
        const scope = sep === -1 ? '' : key.slice(0, sep);
        return { key, pane: { ...store.panes[key], key, scope } as Pane };
      })
      .filter((it) => isResource(it.key, it.pane))
      .sort((a, b) => (a.pane.created_at ?? 0) - (b.pane.created_at ?? 0) || (a.key < b.key ? -1 : 1)),
  );

  // Automatic tiling: walk the list splitting the *longer* side of the remaining
  // area each time, so tiles stay close to square and the last one takes whatever
  // is left. Percentages (0–100) of the stage; the template insets each by a gap.
  interface Rect {
    x: number;
    y: number;
    w: number;
    h: number;
  }
  function tile(n: number): Rect[] {
    const rects: Rect[] = [];
    let x = 0;
    let y = 0;
    let w = 100;
    let h = 100;
    for (let i = 0; i < n; i++) {
      if (i === n - 1) {
        rects.push({ x, y, w, h });
        break;
      }
      if (w >= h) {
        rects.push({ x, y, w: w / 2, h });
        x += w / 2;
        w /= 2;
      } else {
        rects.push({ x, y, w, h: h / 2 });
        y += h / 2;
        h /= 2;
      }
    }
    return rects;
  }

  const laid = $derived.by(() => {
    const rects = tile(resources.length);
    return resources.map((it, i) => ({ ...it, rect: rects[i] }));
  });

  function ledLive(p: Pane): boolean {
    return (p.kind ?? 'data') === 'terminal' ? p.alive !== false : true;
  }
  function ledErr(p: Pane): boolean {
    return (p.kind ?? 'data') === 'terminal' && p.alive === false;
  }
  // The right-aligned tag: a terminal's geometry, else the resource kind (its
  // subtitle, set from `res.kind` by the bridge).
  function tag(p: Pane): string {
    if ((p.kind ?? 'data') === 'terminal') return `${p.rows ?? '?'}×${p.cols ?? '?'}`;
    return p.subtitle || p.kind || 'html';
  }

  // Selection drives the keyboard. `o`/Enter opens the selected resource
  // fullscreen (the same FocusView the feed/board cards use).
  let selectedKey = $state<string | null>(null);
  $effect(() => {
    if (laid.length === 0) selectedKey = null;
    else if (!laid.some((t) => t.key === selectedKey)) selectedKey = laid[0].key;
  });
  function selectIndex(i: number): void {
    if (!laid.length) return;
    selectedKey = laid[Math.max(0, Math.min(laid.length - 1, i))].key;
  }
  function move(delta: number): void {
    const i = laid.findIndex((t) => t.key === selectedKey);
    selectIndex((i < 0 ? 0 : i) + delta);
  }

  onMount(() => {
    setListNav({
      move,
      top: () => selectIndex(0),
      bottom: () => selectIndex(laid.length - 1),
      open: () => {
        if (selectedKey) focusPane(selectedKey);
      },
    });
    return () => setListNav(null);
  });
</script>

<div class="wm">
  {#if laid.length === 0}
    <div class="wm-empty">
      {store.live ? 'no live resources' : 'connecting…'}
      <div class="wm-hint">terminals and browsers tile here as they start</div>
    </div>
  {:else}
    {#each laid as t (t.key)}
      {@const p = t.pane}
      {@const isTerm = (p.kind ?? 'data') === 'terminal'}
      {@const Body = rendererFor(p.kind, p.renderer)}
      <!-- The cell is the exact tiling rect; the inner window is inset by the gap
           so windows never touch. -->
      <div
        class="wm-cell"
        style="left:{t.rect.x}%; top:{t.rect.y}%; width:{t.rect.w}%; height:{t.rect.h}%;"
      >
        <section class="tile" class:sel={selectedKey === t.key}>
          <!-- The title bar doubles as the selector; the ⤢ opens the resource
               fullscreen. Selecting via the bar keeps mouse and keyboard agreed. -->
          <button class="tile-head" onclick={() => (selectedKey = t.key)} title={p.title}>
            <span class="tile-led" class:live={ledLive(p)} class:err={ledErr(p)}></span>
            <span class="tile-title">{p.title || '(resource)'}</span>
            <span class="tile-spacer"></span>
            <span class="tile-tag">{tag(p)}</span>
          </button>
          <button
            class="tile-open"
            aria-label="open fullscreen"
            title="open fullscreen"
            onclick={() => focusPane(t.key)}>⤢</button
          >
          <div class="pane tile-pane" class:term={isTerm}>
            <div class="body" class:term-body={isTerm} class:html-body={p.kind === 'html'}>
              <Body pane={p} />
            </div>
          </div>
        </section>
      </div>
    {/each}
  {/if}
</div>

<style>
  .wm {
    position: relative;
    flex: 1 1 auto;
    min-height: 0;
    overflow: hidden;
    background: var(--bg);
    --gap: 5px;
  }
  .wm-empty {
    position: absolute;
    inset: 0;
    display: grid;
    place-content: center;
    text-align: center;
    color: var(--ink-dim);
    font-family: var(--mono);
    font-size: 13px;
  }
  .wm-hint {
    color: var(--ink-faint);
    font-size: 12px;
    margin-top: 8px;
  }

  /* The tiling cell: the exact percentage rect from the layout. (A distinct name
     from the feed's global `.cell`, whose max-height would clip it.) */
  .wm-cell {
    position: absolute;
  }
  /* The window inside the cell, inset by the gap on every side so windows never
     touch each other or the stage edge. */
  .tile {
    position: absolute;
    inset: var(--gap);
    display: flex;
    flex-direction: column;
    background: var(--panel);
    border: 1px solid var(--edge);
    border-radius: 9px;
    overflow: hidden;
    transition: border-color 0.12s ease;
  }
  .tile.sel {
    border-color: var(--accent);
  }
  .tile-head {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 7px 34px 7px 11px;
    font: inherit;
    text-align: left;
    color: var(--ink);
    background: none;
    border: 0;
    border-bottom: 1px solid var(--edge);
    cursor: pointer;
  }
  .tile-led {
    flex: none;
    width: 7px;
    height: 7px;
    border-radius: 50%;
    background: var(--ink-faint);
  }
  .tile-led.live {
    background: var(--live);
  }
  .tile-led.err {
    background: var(--dead);
  }
  .tile-title {
    flex: 0 1 auto;
    font-family: var(--mono);
    font-size: 12px;
    font-weight: 500;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .tile-spacer {
    flex: 1 1 auto;
  }
  .tile-tag {
    flex: none;
    font-family: var(--mono);
    font-size: 11px;
    color: var(--ink-dim);
    font-variant-numeric: tabular-nums;
    white-space: nowrap;
  }
  /* The fullscreen affordance, parked at the head's right edge. */
  .tile-open {
    position: absolute;
    top: 5px;
    right: 7px;
    width: 22px;
    height: 22px;
    display: grid;
    place-content: center;
    font-size: 12px;
    color: var(--ink-faint);
    background: none;
    border: 0;
    border-radius: 6px;
    cursor: pointer;
  }
  .tile-open:hover {
    color: var(--ink);
    background: var(--elev, var(--panel));
  }

  /* The body fills the rest of the tile. Reuse the renderer CSS scoped under
     `.pane` (terminal spans, cursor, the html frame) but drop the card chrome and
     fixed sizing so the content fills the window and scrolls if it overflows. */
  .tile-pane {
    flex: 1 1 auto;
    min-height: 0;
    border: 0;
    background: transparent;
    cursor: default;
    user-select: text;
  }
  .tile-pane > .body {
    height: 100%;
    overflow: auto;
  }
  .tile :global(.pane pre) {
    height: 100%;
    box-sizing: border-box;
    white-space: pre;
    overflow: auto;
  }
  .tile :global(.body.html-body) {
    height: 100%;
  }
</style>
