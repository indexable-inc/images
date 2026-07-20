<script lang="ts">
  import { encodeKey } from '$lib/keys';
  import { spanStyle } from '$lib/render';
  import type { TermConnection } from '$lib/term.svelte';

  const { conn }: { conn: TermConnection } = $props();

  let containerEl = $state<HTMLDivElement | undefined>();
  let probeEl = $state<HTMLSpanElement | undefined>();
  let cellW = $state(0);
  let cellH = $state(0);
  let availW = $state(0);
  let availH = $state(0);

  function px(n: number): string {
    return `${String(n)}px`;
  }

  // Measure one cell from a 10-glyph probe for sub-pixel accuracy.
  $effect(() => {
    const el = probeEl;
    if (el === undefined) {
      return;
    }
    const rect = el.getBoundingClientRect();
    cellW = rect.width / 10;
    cellH = rect.height;
  });

  $effect(() => {
    const el = containerEl;
    if (el === undefined) {
      return;
    }
    const observer = new ResizeObserver((entries) => {
      for (const entry of entries) {
        availW = entry.contentRect.width;
        availH = entry.contentRect.height;
      }
    });
    observer.observe(el);
    return () => {
      observer.disconnect();
    };
  });

  // Grab keyboard focus on mount (i.e. on every session switch).
  $effect(() => {
    containerEl?.focus();
  });

  // Driver-owns-size: only the driver translates its container size into a
  // resize request, debounced so drags do not spam the PTY.
  $effect(() => {
    const driving = conn.isDriver;
    const w = availW;
    const h = availH;
    const cw = cellW;
    const ch = cellH;
    if (!driving || cw <= 0 || ch <= 0 || w <= 0 || h <= 0) {
      return;
    }
    const cols = Math.max(2, Math.floor(w / cw));
    const rows = Math.max(2, Math.floor(h / ch));
    const timer = setTimeout(() => {
      if (cols !== conn.seatCols || rows !== conn.seatRows) {
        conn.sendResize(cols, rows);
      }
    }, 200);
    return () => {
      clearTimeout(timer);
    };
  });

  const gridW = $derived(conn.cols * cellW);
  const gridH = $derived(conn.rows * cellH);
  // Viewers see the driver-sized grid scaled down to fit; never scaled up.
  const scale = $derived(
    conn.isDriver || gridW <= 0 || gridH <= 0 || availW <= 0 || availH <= 0
      ? 1
      : Math.min(1, availW / gridW, availH / gridH)
  );

  function onKeydown(ev: KeyboardEvent): void {
    const data = encodeKey(ev, conn.appCursor);
    if (data !== null) {
      ev.preventDefault();
      conn.sendInput(data);
    }
  }

  function onPaste(ev: ClipboardEvent): void {
    const text = ev.clipboardData?.getData('text') ?? '';
    if (text.length > 0) {
      ev.preventDefault();
      conn.sendInput(text);
    }
  }
</script>

<div
  class="terminal"
  bind:this={containerEl}
  tabindex="0"
  role="textbox"
  aria-multiline="true"
  aria-label="terminal"
  onkeydown={onKeydown}
  onpaste={onPaste}
  onclick={() => {
    containerEl?.focus();
  }}
>
  <span class="probe" bind:this={probeEl} aria-hidden="true">WWWWWWWWWW</span>
  <div
    class="grid"
    style:width={px(gridW)}
    style:height={px(gridH)}
    style:transform={`scale(${String(scale)})`}
  >
    {#each conn.lines as spans, y (y)}
      <div class="row" style:top={px(y * cellH)} style:height={px(cellH)}>
        {#each spans as span, i (i)}
          <span style={spanStyle(span)}>{span.text}</span>
        {/each}
      </div>
    {/each}
    {#if conn.cursor !== null && conn.cursor.visible}
      <div
        class="cursor {conn.cursor.shape}"
        style:left={px(conn.cursor.x * cellW)}
        style:top={px(conn.cursor.y * cellH)}
        style:width={px(cellW)}
        style:height={px(cellH)}
      ></div>
    {/if}
  </div>
</div>

<style>
  .terminal {
    position: relative;
    flex: 1;
    min-width: 0;
    overflow: hidden;
    background: #111;
    outline: none;
    padding: 0;
    line-height: 1.25;
    cursor: text;
  }

  .probe {
    position: absolute;
    visibility: hidden;
    white-space: pre;
  }

  .grid {
    position: absolute;
    top: 0;
    left: 0;
    transform-origin: top left;
  }

  .row {
    position: absolute;
    left: 0;
    white-space: pre;
    color: #ddd;
    overflow: hidden;
  }

  .cursor {
    position: absolute;
    pointer-events: none;
  }

  .cursor.block {
    background: #ddd;
    mix-blend-mode: difference;
  }

  .cursor.bar {
    border-left: 2px solid #ddd;
  }

  .cursor.underline {
    border-bottom: 2px solid #ddd;
  }

  .cursor.hollow {
    border: 1px solid #ddd;
  }
</style>
