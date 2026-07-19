<script lang="ts">
  import type { ScoreDimension, ScoredItem } from './scores';

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

  let xKey = $state(initialX);
  let yKey = $state(initialY);

  function dimension(key: string): ScoreDimension {
    const found = dimensions.find((d) => d.key === key);
    if (found === undefined) throw new Error(`unknown dimension '${key}'`);
    return found;
  }

  const xDim = $derived(dimension(xKey));
  const yDim = $derived(dimension(yKey));

  const W = 680;
  const H = 460;
  const PAD = { top: 18, right: 24, bottom: 44, left: 44 };

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
      const k = `${String(item.scores[xKey].value)}/${String(item.scores[yKey].value)}`;
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
          px: sx(item.scores[xKey].value) + r * Math.cos(angle),
          py: sy(item.scores[yKey].value) + r * Math.sin(angle)
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
    <svg viewBox="0 0 {W} {H}" role="img" aria-label="Items plotted by {xDim.label} against {yDim.label}">
      {#each [1, 4, 7, 10] as v (v)}
        <line class="grid" x1={sx(v)} y1={sy(1)} x2={sx(v)} y2={sy(10)} />
        <line class="grid" x1={sx(1)} y1={sy(v)} x2={sx(10)} y2={sy(v)} />
      {/each}

      <text class="axis-end" x={sx(1)} y={H - 14} text-anchor="start">{xDim.low}</text>
      <text class="axis-end" x={sx(10)} y={H - 14} text-anchor="end">{xDim.high} →</text>
      <text class="axis-end" x={12} y={sy(1)} transform="rotate(-90, 12, {sy(1)})" text-anchor="start">
        {yDim.low}
      </text>
      <text class="axis-end" x={12} y={sy(10)} transform="rotate(-90, 12, {sy(10)})" text-anchor="end">
        {yDim.high} →
      </text>

      {#each points as point (point.item.id)}
        <!-- Callers pass resolve()d hrefs; a generic component cannot resolve. -->
        <!-- eslint-disable-next-line svelte/no-navigation-without-resolve -->
        <a href={point.item.href}>
          <g
            class="mark"
            class:dimmed={hovered !== null && hovered !== point}
            onpointerenter={() => (hovered = point)}
            onpointerleave={() => (hovered = null)}
          >
            <circle class="hit" cx={point.px} cy={point.py} r="14" />
            <circle class="dot" cx={point.px} cy={point.py} r="6" style:--c={point.item.colorVar} />
            <text class="mark-label" x={point.px + 10} y={point.py + 3.5}>{point.item.label}</text>
          </g>
        </a>
      {/each}
    </svg>

    {#if hovered}
      <div
        class="tooltip"
        style:left={`${((hovered.px / W) * 100).toFixed(2)}%`}
        style:top={`${((hovered.py / H) * 100).toFixed(2)}%`}
      >
        <strong>{hovered.item.label}</strong> · {hovered.item.title}
        <span class="detail">{hovered.item.detail}</span>
        <span class="why">
          {xDim.label} {hovered.item.scores[xKey].value}/10: {hovered.item.scores[xKey].why}
        </span>
        <span class="why">
          {yDim.label} {hovered.item.scores[yKey].value}/10: {hovered.item.scores[yKey].why}
        </span>
      </div>
    {/if}
  </div>
</figure>

<style>
  .chart {
    margin: 1.5rem 0 2rem;
  }

  .controls {
    display: flex;
    align-items: center;
    gap: 1rem;
    flex-wrap: wrap;
    margin-bottom: 0.5rem;
  }

  .controls label {
    display: inline-flex;
    align-items: center;
    gap: 0.4rem;
    font-family: var(--font-mono);
    font-size: 0.75rem;
    color: var(--fg-muted);
    text-transform: uppercase;
  }

  .controls select {
    font: inherit;
    color: var(--fg);
    background: var(--bg);
    border: 1px solid var(--rule);
    border-radius: 6px;
    padding: 0.25rem 0.5rem;
  }

  .plot {
    position: relative;
    border: 1px solid var(--rule);
    border-radius: 10px;
    overflow: hidden;
  }

  svg {
    display: block;
    width: 100%;
    height: auto;
  }

  .grid {
    stroke: var(--rule);
    stroke-width: 1;
  }

  .axis-end {
    font-family: var(--font-mono);
    font-size: 11px;
    fill: var(--fg-faint);
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

  .dot {
    fill: var(--c);
    /* Surface ring keeps overlapping dots separable. */
    stroke: var(--bg);
    stroke-width: 2;
  }

  .mark-label {
    font-family: var(--font-mono);
    font-size: 11px;
    fill: var(--fg-muted);
  }

  .tooltip {
    position: absolute;
    transform: translate(12px, -50%);
    max-width: 22rem;
    background: var(--bg);
    border: 1px solid var(--rule);
    border-radius: 8px;
    padding: 0.45rem 0.65rem;
    font-size: 0.8rem;
    pointer-events: none;
    box-shadow: 0 2px 10px rgb(0 0 0 / 0.12);
  }

  .tooltip .detail,
  .tooltip .why {
    display: block;
    color: var(--fg-muted);
    margin-top: 0.15rem;
  }
</style>
