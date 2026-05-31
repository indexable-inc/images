<script lang="ts">
  import { untrack } from 'svelte';
  import { renderInto, hasOutput, type Cursor } from '$lib/ansi';
  import { metrics } from '$lib/metrics.svelte';
  import type { Term } from '$lib/types';

  const FONT = 12;
  const CHROME_X = 12 * 2 + 1 * 2; // pre padding-inline (12) + card border (1), both sides

  let { term, grabbed = false }: { term: Term; grabbed?: boolean } = $props();
  let preEl: HTMLElement | undefined = $state();

  const cols = $derived(term.cols && term.cols > 0 ? term.cols : 80);
  const alive = $derived(term.alive !== false);
  const screen = $derived(term.screen ?? '');
  const output = $derived(hasOutput(screen));
  const label = $derived([term.command, term.args].filter(Boolean).join(' ') || '(terminal)');
  const showChip = $derived(!!term.scope && term.scope !== 'local');
  // The card is exactly its terminal's natural width, so the screen never needs
  // to scroll or shrink; the board's pan/zoom handles a terminal too big to see.
  const width = $derived(Math.ceil(cols * FONT * metrics.ratio) + CHROME_X);
  const cursor = $derived<Cursor | null>(
    alive && output && term.cursor_visible !== false && typeof term.cursor_row === 'number'
      ? { row: term.cursor_row, col: term.cursor_col ?? 0, shape: term.cursor_shape ?? 'block' }
      : null,
  );
  // A stable string key for the cursor: `cursor` is a fresh object each frame,
  // so depending on it would re-run the repaint for every card on every frame.
  // Depending on this primitive instead skips frames where nothing moved.
  const cursorKey = $derived(cursor ? `${cursor.row},${cursor.col},${cursor.shape}` : '');

  // Repaint the screen imperatively (the spans are built in JS, not the
  // template). Track only the primitives that should trigger a repaint; read the
  // live screen/cursor objects untracked so the per-frame object churn in the
  // store does not thrash a full replaceChildren on unchanged cards.
  $effect(() => {
    void screen;
    void cursorKey;
    void output;
    void alive;
    void term.exit_code;
    void metrics.themeV;
    const el = preEl;
    if (!el) return;
    untrack(() => {
      if (output) {
        renderInto(el, screen, cursor);
      } else {
        const ph = document.createElement('span');
        ph.className = 'placeholder';
        ph.textContent = alive
          ? '· no output'
          : typeof term.exit_code === 'number'
            ? `· exited (code ${term.exit_code})`
            : '· exited';
        el.replaceChildren(ph);
      }
    });
  });
</script>

<div class="term" class:dead={!alive} class:grabbed style="width: {width}px; font-size: {FONT}px;">
  <div class="head" data-drag-handle>
    <span class="led" title={alive ? 'running' : 'exited'}></span>
    <span class="cmd" title={label}>{label}</span>
    <span class="spacer"></span>
    {#if showChip}<span class="chip" title={'producer ' + term.scope}>{term.scope}</span>{/if}
    <span class="size">{term.rows}×{term.cols}</span>
  </div>
  <pre bind:this={preEl}></pre>
</div>
