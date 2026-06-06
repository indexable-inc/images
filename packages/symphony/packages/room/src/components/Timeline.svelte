<script lang="ts">
  // Left-gutter timeline rail.
  //
  // One tick per top-level block in the transcript (user / assistant /
  // system / tool-work-group), positioned vertically by the block's
  // timestamp relative to the thread's [start, end] span. Clicking
  // scrolls the matching message into view; hovering shows the local
  // time. With a long thread the rail makes the *rhythm* of the
  // conversation visible (bursts cluster, gaps spread), and the
  // user can jump to any point in time.

  import { humanAgo, absoluteTime } from '$lib/time';
  import { nowTick } from '$lib/activity';

  export interface TimelineEntry {
    id: string;
    ts_ms: number;
    kind: 'user' | 'assistant' | 'system' | 'work';
  }

  interface Props {
    entries: TimelineEntry[];
    /** Scroll container that holds the messages — we look up the
     *  data-message-id inside it and scrollIntoView. */
    scroller?: HTMLElement;
  }

  let { entries, scroller }: Props = $props();

  // Second-resolution clock so the hover tooltip's "X minutes ago"
  // refreshes between paints without depending on a fresh mouseover.
  let now = $state(Date.now());
  const unsubNow = nowTick.subscribe((v) => (now = v));
  $effect(() => () => unsubNow());

  // Time span of the thread. Empty/single-message threads collapse to
  // zero range; we handle that by pinning every tick to the middle.
  let span = $derived.by(() => {
    if (entries.length === 0) return { min: 0, max: 0, range: 0 };
    let min = entries[0]!.ts_ms;
    let max = min;
    for (const e of entries) {
      if (e.ts_ms < min) min = e.ts_ms;
      if (e.ts_ms > max) max = e.ts_ms;
    }
    return { min, max, range: max - min };
  });

  function pctFor(ts: number): number {
    if (span.range <= 0) return 50;
    return ((ts - span.min) / span.range) * 100;
  }

  function jumpTo(id: string) {
    if (!scroller) return;
    const el = scroller.querySelector<HTMLElement>(`[data-message-id="${CSS.escape(id)}"]`);
    if (el) el.scrollIntoView({ behavior: 'smooth', block: 'center' });
  }

  // Custom tooltip state. We avoid the native `title=` attribute so the
  // hover affordance can match the rest of the app's chrome (rounded
  // chip, var(--bg-elev), monospace digits) instead of the OS default
  // which collides with the dark rail visually and lags ~500ms.
  let hovered = $state<{ ts: number; top: number; left: number } | null>(null);

  function showTooltip(e: PointerEvent, ts: number) {
    const rect = (e.currentTarget as HTMLElement).getBoundingClientRect();
    hovered = {
      ts,
      top: rect.top + rect.height / 2,
      left: rect.right
    };
  }
  function hideTooltip() {
    hovered = null;
  }
</script>

{#if entries.length > 0}
  <div class="rail" aria-label="Timeline">
    <div class="track"></div>
    {#each entries as e (e.id)}
      <button
        type="button"
        class="tick {e.kind}"
        style="top: {pctFor(e.ts_ms)}%"
        onclick={() => jumpTo(e.id)}
        onpointerenter={(ev) => showTooltip(ev, e.ts_ms)}
        onpointerleave={hideTooltip}
        onfocus={(ev) => showTooltip(ev as unknown as PointerEvent, e.ts_ms)}
        onblur={hideTooltip}
        aria-label={`Jump to message — ${humanAgo(e.ts_ms, now)} (${absoluteTime(e.ts_ms)})`}
      ></button>
    {/each}
  </div>
{/if}

{#if hovered}
  <div
    class="tick-tooltip"
    role="tooltip"
    style="top: {hovered.top}px; left: {hovered.left}px;"
  >
    {humanAgo(hovered.ts, now)}
  </div>
{/if}

<style>
  .rail {
    position: absolute;
    top: 16px;
    bottom: 18px;
    left: 6px;
    width: 14px;
    pointer-events: none;
    z-index: 4;
  }
  .track {
    position: absolute;
    top: 0;
    bottom: 0;
    left: 50%;
    width: 1px;
    transform: translateX(-50%);
    background: color-mix(in srgb, var(--border) 80%, transparent);
    border-radius: 999px;
  }
  .tick {
    position: absolute;
    left: 50%;
    width: 6px;
    height: 6px;
    margin: -3px 0 0 -3px;
    border-radius: 999px;
    border: 0;
    padding: 0;
    background: var(--text-dim);
    pointer-events: auto;
    cursor: pointer;
    transition: transform 0.12s, background 0.12s, box-shadow 0.12s;
  }
  .tick:hover,
  .tick:focus-visible {
    background: var(--accent);
    transform: scale(1.6);
    box-shadow: 0 0 0 4px color-mix(in srgb, var(--accent) 18%, transparent);
    outline: none;
  }
  /* Kind-specific colour so the rail also reads as "who said what". */
  .tick.user {
    background: color-mix(in srgb, var(--text-strong) 70%, transparent);
  }
  .tick.assistant {
    background: color-mix(in srgb, var(--text-dim) 85%, transparent);
  }
  .tick.system {
    background: color-mix(in srgb, var(--text-dim) 50%, transparent);
    width: 5px;
    height: 5px;
    margin: -2.5px 0 0 -2.5px;
  }
  .tick.work {
    background: color-mix(in srgb, var(--accent) 55%, transparent);
    width: 5px;
    height: 5px;
    margin: -2.5px 0 0 -2.5px;
  }

  /* Fixed-position chip anchored to the right edge of the hovered
     tick. `transform` recenters vertically and nudges past the tick's
     focus ring (4px). `pointer-events: none` keeps it from stealing
     the hover and starting a flicker loop with the underlying tick. */
  .tick-tooltip {
    position: fixed;
    z-index: 1000;
    transform: translate(10px, -50%);
    background: var(--bg-elev, var(--bg-pill));
    border: 1px solid var(--border-hi, var(--border));
    border-radius: 6px;
    padding: 4px 8px;
    box-shadow: 0 6px 18px rgba(0, 0, 0, 0.18);
    color: var(--text-strong);
    font-size: 11.5px;
    line-height: 1.2;
    font-variant-numeric: tabular-nums;
    font-feature-settings: 'tnum';
    white-space: nowrap;
    pointer-events: none;
  }
</style>
