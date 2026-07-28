<script lang="ts">
  import { untrack } from 'svelte';
  import { renderInto, hasOutput, type Cursor } from '$lib/ansi';
  import { metrics } from '$lib/metrics.svelte';
  import type { Pane } from '$lib/types';

  // The terminal renderer: `scrollback` is plain text and `body` is the ANSI-SGR
  // viewport. Repaint
  // imperatively (spans are built in JS) and track only the primitives that
  // should trigger a repaint, so per-frame object churn does not thrash a full
  // replaceChildren on unchanged cards.
  let { pane }: { pane: Pane } = $props();
  let preEl: HTMLElement | undefined = $state();

  const alive = $derived(pane.alive !== false);
  const screen = $derived(pane.body ?? '');
  const scrollback = $derived(pane.scrollback ?? '');
  const scrollbackRows = $derived(scrollback ? scrollback.split('\n').length : 0);
  const fullScreen = $derived(scrollback ? `${scrollback}\n${screen}` : screen);
  const output = $derived(hasOutput(fullScreen));
  const cursor = $derived<Cursor | null>(
    alive && output && pane.cursor_visible !== false && typeof pane.cursor_row === 'number'
      ? { row: pane.cursor_row + scrollbackRows, col: pane.cursor_col ?? 0, shape: pane.cursor_shape ?? 'block' }
      : null,
  );
  // A stable string key for the cursor: `cursor` is a fresh object each frame, so
  // depending on it would re-run the repaint for every card on every frame.
  const cursorKey = $derived(cursor ? `${cursor.row},${cursor.col},${cursor.shape}` : '');

  $effect(() => {
    void screen;
    void scrollback;
    void cursorKey;
    void output;
    void alive;
    void pane.exit_code;
    void metrics.themeV;
    const el = preEl;
    if (!el) return;
    untrack(() => {
      if (output) {
        renderInto(el, fullScreen, cursor);
      } else {
        const ph = document.createElement('span');
        ph.className = 'placeholder';
        ph.textContent = alive
          ? '· no output'
          : typeof pane.exit_code === 'number'
            ? `· exited (code ${pane.exit_code})`
            : '· exited';
        el.replaceChildren(ph);
      }
    });
  });
</script>

<pre bind:this={preEl}></pre>
