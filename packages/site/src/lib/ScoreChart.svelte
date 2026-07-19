<script lang="ts">
  import { scoreOf, type ScoreDimension, type ScoredItem } from './scores';

  const {
    items,
    dimensions,
    initialX,
    initialY
  }: {
    items: ScoredItem[];
    dimensions: readonly ScoreDimension[];
    initialX: string;
    initialY: string;
  } = $props();

  // The initial props seed the selection once; the selects own it after.
  // svelte-ignore state_referenced_locally
  let xKey = $state(initialX);
  // svelte-ignore state_referenced_locally
  let yKey = $state(initialY);

  function dimension(key: string): ScoreDimension {
    const found = dimensions.find((d) => d.key === key);
    if (found === undefined) throw new Error(`unknown dimension '${key}'`);
    return found;
  }

  const xDim = $derived(dimension(xKey));
  const yDim = $derived(dimension(yKey));

  // Fixed 1:1 pixels on the page's cell grid: H and the vertical pads are
  // whole multiples of the 21px --cell-h, and W fits the 72ch content column
  // so the chart renders unscaled with the site's one 14px cell.
  const W = 588;
  const H = 462;
  const PAD = { top: 21, right: 21, bottom: 42, left: 42 };

  const plotW = W - PAD.left - PAD.right;
  const plotH = H - PAD.top - PAD.bottom;

  function sx(v: number): number {
    return PAD.left + ((v - 1) / 9) * plotW;
  }
  function sy(v: number): number {
    return H - PAD.bottom - ((v - 1) / 9) * plotH;
  }

  type Point = { item: ScoredItem; px: number; py: number };

  // Coincident points fan out in a small ring so every dot stays hittable.
  const points = $derived.by(() => {
    const groups: Record<string, ScoredItem[]> = {};
    for (const item of items) {
      const k = `${String(scoreOf(item.id, item.scores, xKey).value)}/${String(scoreOf(item.id, item.scores, yKey).value)}`;
      groups[k] = [...(groups[k] ?? []), item];
    }
    const out: Point[] = [];
    for (const members of Object.values(groups)) {
      members.forEach((item, i) => {
        // Offset start angle so a pair separates vertically, keeping their
        // side-mounted labels from colliding.
        const angle = (i / members.length) * 2 * Math.PI - Math.PI / 4;
        const r = members.length > 1 ? 8 : 0;
        out.push({
          item,
          px: sx(scoreOf(item.id, item.scores, xKey).value) + r * Math.cos(angle),
          py: sy(scoreOf(item.id, item.scores, yKey).value) + r * Math.sin(angle)
        });
      });
    }
    return out;
  });

  let hovered = $state<Point | null>(null);
</script>

<figure class="chart">
  <div class="controls">
    <label>
      x
      <select bind:value={xKey}>
        {#each dimensions as dim (dim.key)}
          <option value={dim.key} disabled={dim.key === yKey}>{dim.label}</option>
        {/each}
      </select>
    </label>
    <label>
      y
      <select bind:value={yKey}>
        {#each dimensions as dim (dim.key)}
          <option value={dim.key} disabled={dim.key === xKey}>{dim.label}</option>
        {/each}
      </select>
    </label>
  </div>

  <div class="plot">
    <svg width={W} height={H} viewBox="0 0 {W} {H}" role="img" aria-label="Items plotted by {xDim.label} against {yDim.label}">
      {#each [1, 4, 7, 10] as v (v)}
        <line class="grid" x1={sx(v)} y1={sy(1)} x2={sx(v)} y2={sy(10)} />
        <line class="grid" x1={sx(1)} y1={sy(v)} x2={sx(10)} y2={sy(v)} />
      {/each}

      <text class="axis-end" x={sx(1)} y={H - 6} text-anchor="start">{xDim.low}</text>
      <text class="axis-end" x={sx(10)} y={H - 6} text-anchor="end">{xDim.high} →</text>
      <text class="axis-end" x={14} y={sy(1)} transform="rotate(-90, 14, {sy(1)})" text-anchor="start">
        {yDim.low}
      </text>
      <text class="axis-end" x={14} y={sy(10)} transform="rotate(-90, 14, {sy(10)})" text-anchor="end">
        {yDim.high} →
      </text>

      {#each points as point (point.item.id)}
        <!-- Callers pass resolve()d hrefs; a generic component cannot resolve. -->
        <!-- eslint-disable svelte/no-navigation-without-resolve -->
        <a
          onpointerenter={() => (hovered = point)}
          onpointerleave={() => (hovered = null)}
          href={point.item.href}
        >
          <g class="mark" class:dimmed={hovered !== null && hovered !== point}>
            <circle class="hit" cx={point.px} cy={point.py} r="14" />
            <rect class="dot" x={point.px - 5} y={point.py - 5} width="10" height="10" style:--c={point.item.colorVar} />
            <text class="mark-label" x={point.px + 10} y={point.py + 5}>{point.item.label}</text>
          </g>
        </a>
        <!-- eslint-enable svelte/no-navigation-without-resolve -->
      {/each}
    </svg>

    {#if hovered}
      <div
        class="tooltip"
        class:flip={hovered.px > W * 0.55}
        style:left={`${((hovered.px / W) * 100).toFixed(2)}%`}
        style:top={`${((hovered.py / H) * 100).toFixed(2)}%`}
      >
        <strong>{hovered.item.label}</strong> · {hovered.item.title}
        <span class="detail">{hovered.item.detail}</span>
        {#each [xDim, yDim] as dim (dim.key)}
          {@const score = scoreOf(hovered.item.id, hovered.item.scores, dim.key)}
          <span class="why">
            {dim.label} {score.value}/10: {score.why}
          </span>
        {/each}
      </div>
    {/if}
  </div>
</figure>

<style>
  .chart {
    margin: var(--cell-h) 0 calc(var(--cell-h) * 2);
  }

  .controls {
    display: flex;
    align-items: center;
    gap: 0 2ch;
    flex-wrap: wrap;
    margin-bottom: var(--cell-h);
  }

  .controls label {
    display: inline-flex;
    align-items: center;
    gap: 1ch;
    color: var(--fg-muted);
    text-transform: uppercase;
    letter-spacing: 0.04em;
  }

  /* TUI dropdown: a bracketed frame around the current value. */
  .controls select {
    font: inherit;
    /* Chrome forces line-height: normal on native selects; pin the box to
       one cell so the control still sits on the grid. */
    height: var(--cell-h);
    color: var(--fg);
    background: var(--bg);
    border: 1px solid var(--rule);
    border-radius: var(--radius);
    padding: 0 1ch;
  }

  .controls select:hover {
    border-color: var(--fg-muted);
  }

  .plot {
    position: relative;
    /* The frame's top and bottom 1px rules take no grid space: pull the
       figure's flow height back by 2px here (a margin on the figure would
       collapse with the next block's margin and lose the compensation). */
    margin-bottom: -2px;
    width: max-content;
    max-width: 100%;
    border: 1px solid var(--rule);
    border-radius: var(--radius);
    /* No overflow clipping: the tooltip must escape the chart box. */
  }

  svg {
    display: block;
    /* Downscale only when the viewport is narrower than the chart; the
       tooltip's percentage anchors keep tracking the marks. */
    max-width: 100%;
    height: auto;
  }

  /* Dotted rules, like box-drawing fill in a TUI plot. */
  .grid {
    stroke: var(--rule);
    stroke-width: 1;
    stroke-dasharray: 1 3;
  }

  .axis-end {
    font-family: var(--font-mono);
    font-size: 14px;
    fill: var(--fg-faint);
    letter-spacing: 0.04em;
    text-transform: uppercase;
  }

  .mark {
    cursor: pointer;
  }

  .mark.dimmed {
    opacity: 0.35;
  }

  .hit {
    fill: transparent;
  }

  /* Square cell marks, not dots: the plot reads as painted character
     cells. The ground-colored ring keeps overlapping marks separable. */
  .dot {
    fill: var(--c);
    stroke: var(--bg);
    stroke-width: 2;
  }

  .mark-label {
    font-family: var(--font-mono);
    font-size: 14px;
    fill: var(--fg-muted);
  }

  .tooltip {
    position: absolute;
    transform: translate(2ch, -50%);
    width: max-content;
    max-width: min(52ch, 60vw);
    z-index: 1;
    background: var(--bg);
    border: 1px solid var(--fg-muted);
    border-radius: var(--radius);
    padding: 0 1ch;
    pointer-events: none;
  }

  /* Right-half marks open the tooltip to the left so it never runs off page. */
  .tooltip.flip {
    transform: translate(calc(-100% - 2ch), -50%);
  }

  .tooltip .detail,
  .tooltip .why {
    display: block;
    color: var(--fg-muted);
  }
</style>
