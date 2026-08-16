<script lang="ts">
  import { SvelteMap } from 'svelte/reactivity';
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

  // The map viewport, in viewBox units. The panned/zoomed world is clamped so
  // content always covers this rect (a bit of the plot is always on screen).
  const vL = PAD.left;
  const vR = W - PAD.right;
  const vT = PAD.top;
  const vB = H - PAD.bottom;

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

  // --- Direct-manipulation transform (Apple-Maps feel) --------------------
  // World -> view is one similarity transform in viewBox units:
  //   viewX = k * baseX + panX. Only positions transform; marker size, stroke
  //   and label size stay constant (semantic zoom). Gesture handlers write
  //   k/panX/panY straight from the event, so there is no easing mid-gesture
  //   and no $effect that both reads and writes the transform (which loops).
  const MIN_K = 1;
  const MAX_K = 8;

  let k = $state(1);
  let panX = $state(0);
  let panY = $state(0);

  const transformed = $derived(k !== 1 || panX !== 0 || panY !== 0);

  function tx(x: number): number {
    return k * x + panX;
  }
  function ty(y: number): number {
    return k * y + panY;
  }

  // Content base bounds equal the plot rect, so clamp the pan to keep the
  // transformed rect covering the viewport: no empty gutters, ever.
  function clampPan(): void {
    panX = Math.min(vL * (1 - k), Math.max(vR * (1 - k), panX));
    panY = Math.min(vT * (1 - k), Math.max(vB * (1 - k), panY));
  }

  let svgEl: SVGSVGElement | undefined = $state();
  let plotEl: HTMLDivElement | undefined = $state();

  // Client pixels -> viewBox units, honouring any responsive downscale.
  function toView(clientX: number, clientY: number): { x: number; y: number } {
    if (svgEl === undefined) return { x: 0, y: 0 };
    const r = svgEl.getBoundingClientRect();
    return { x: ((clientX - r.left) / r.width) * W, y: ((clientY - r.top) / r.height) * H };
  }

  function viewScale(): number {
    if (svgEl === undefined) return 1;
    return W / svgEl.getBoundingClientRect().width;
  }

  // Zoom by `factor` about a viewBox anchor, keeping the world point under the
  // anchor fixed: cursor/centroid-anchored zoom.
  function zoomAt(ax: number, ay: number, factor: number): void {
    const nk = Math.min(MAX_K, Math.max(MIN_K, k * factor));
    if (nk === k) return;
    const worldX = (ax - panX) / k;
    const worldY = (ay - panY) / k;
    k = nk;
    panX = ax - k * worldX;
    panY = ay - k * worldY;
    clampPan();
  }

  function panBy(dx: number, dy: number): void {
    panX += dx;
    panY += dy;
    clampPan();
  }

  function reset(): void {
    stopInertia();
    k = 1;
    panX = 0;
    panY = 0;
  }

  // --- Pointer gestures ----------------------------------------------------
  const DRAG_THRESHOLD = 4; // client px a press must travel to become a drag
  const pointers = new SvelteMap<number, { x: number; y: number }>();
  let dragging = $state(false);
  let downX = 0;
  let downY = 0;
  let suppressClick = false;
  let pinchPrev: { cx: number; cy: number; dist: number } | null = null;

  // Inertia is the only motion we keep; a flick coasts and decays, and any new
  // contact kills it.
  let velX = 0;
  let velY = 0;
  let lastMoveAt = 0;
  let inertiaRAF = 0;

  function stopInertia(): void {
    if (inertiaRAF !== 0) {
      cancelAnimationFrame(inertiaRAF);
      inertiaRAF = 0;
    }
    velX = 0;
    velY = 0;
  }

  function startInertia(): void {
    if (Math.hypot(velX, velY) < 0.02) return;
    let last = performance.now();
    const step = (): void => {
      const now = performance.now();
      const dt = now - last;
      last = now;
      panBy(velX * dt, velY * dt);
      const decay = Math.pow(0.95, dt / 16);
      velX *= decay;
      velY *= decay;
      if (Math.hypot(velX, velY) < 0.01) {
        inertiaRAF = 0;
        return;
      }
      inertiaRAF = requestAnimationFrame(step);
    };
    inertiaRAF = requestAnimationFrame(step);
  }

  function pinchState(): { cx: number; cy: number; dist: number } {
    const pts = [...pointers.values()];
    const mid = toView((pts[0].x + pts[1].x) / 2, (pts[0].y + pts[1].y) / 2);
    const dist = Math.hypot(pts[0].x - pts[1].x, pts[0].y - pts[1].y);
    return { cx: mid.x, cy: mid.y, dist };
  }

  function onPointerDown(e: PointerEvent): void {
    if ((e.target as Element).closest('.map-controls') !== null) return;
    stopInertia();
    suppressClick = false;
    pointers.set(e.pointerId, { x: e.clientX, y: e.clientY });
    if (pointers.size === 1) {
      downX = e.clientX;
      downY = e.clientY;
      lastMoveAt = performance.now();
    } else if (pointers.size === 2) {
      dragging = true;
      hovered = null;
      pinchPrev = pinchState();
      for (const id of pointers.keys()) {
        try {
          plotEl?.setPointerCapture(id);
        } catch {
          // capture is best-effort
        }
      }
    }
  }

  function onPointerMove(e: PointerEvent): void {
    const prev = pointers.get(e.pointerId);
    if (prev === undefined) return;
    pointers.set(e.pointerId, { x: e.clientX, y: e.clientY });

    if (pointers.size >= 2) {
      if (pinchPrev === null) return;
      const now = pinchState();
      const factor = now.dist === 0 || pinchPrev.dist === 0 ? 1 : now.dist / pinchPrev.dist;
      panBy(now.cx - pinchPrev.cx, now.cy - pinchPrev.cy);
      zoomAt(now.cx, now.cy, factor);
      pinchPrev = now;
      return;
    }

    if (!dragging) {
      if (Math.hypot(e.clientX - downX, e.clientY - downY) <= DRAG_THRESHOLD) return;
      dragging = true;
      hovered = null;
      try {
        plotEl?.setPointerCapture(e.pointerId);
      } catch {
        // capture is best-effort
      }
    }

    const s = viewScale();
    const dvx = (e.clientX - prev.x) * s;
    const dvy = (e.clientY - prev.y) * s;
    panBy(dvx, dvy);

    const now = performance.now();
    const dt = Math.max(1, now - lastMoveAt);
    velX = dvx / dt;
    velY = dvy / dt;
    lastMoveAt = now;
  }

  function onPointerUp(e: PointerEvent): void {
    pointers.delete(e.pointerId);
    try {
      plotEl?.releasePointerCapture(e.pointerId);
    } catch {
      // release is best-effort
    }
    if (pointers.size < 2) pinchPrev = null;
    if (pointers.size === 0) {
      if (dragging) {
        suppressClick = true;
        startInertia();
      }
      dragging = false;
    }
  }

  // A drag ends with a synthetic click on the anchor under the pointer; swallow
  // it in the capture phase so a pan never opens a Plan. A real tap, which
  // never crosses the threshold, leaves the flag clear and clicks through.
  function onClickCapture(e: MouseEvent): void {
    if (suppressClick) {
      e.preventDefault();
      e.stopPropagation();
      suppressClick = false;
    }
  }

  function onDblClick(e: MouseEvent): void {
    if ((e.target as Element).closest('.map-controls') !== null) return;
    e.preventDefault();
    const a = toView(e.clientX, e.clientY);
    zoomAt(a.x, a.y, 1.8);
  }

  // Zoom rides ctrl/cmd+wheel (trackpad pinch arrives as ctrl+wheel in
  // Chrome/Firefox; Safari pinch uses the gesture events wired in the effect
  // below). A plain wheel is the page scrolling and passes through
  // untouched, so hovering the map never traps the reader; panning is the
  // pointer drag. The listener stays non-passive to preventDefault the
  // zoom case.
  function onWheel(e: WheelEvent): void {
    if (!e.ctrlKey && !e.metaKey) return;
    e.preventDefault();
    stopInertia();
    const a = toView(e.clientX, e.clientY);
    zoomAt(a.x, a.y, Math.exp(-e.deltaY * 0.01));
  }

  type GestureLikeEvent = Event & { scale: number; clientX: number; clientY: number };
  let gestureStartK = 1;
  let gestureAnchor = { x: 0, y: 0 };

  function onGestureStart(e: GestureLikeEvent): void {
    e.preventDefault();
    stopInertia();
    gestureStartK = k;
    gestureAnchor = toView(e.clientX, e.clientY);
  }

  function onGestureChange(e: GestureLikeEvent): void {
    e.preventDefault();
    const target = Math.min(MAX_K, Math.max(MIN_K, gestureStartK * e.scale));
    zoomAt(gestureAnchor.x, gestureAnchor.y, target / k);
  }

  function zoomButton(factor: number): void {
    zoomAt((vL + vR) / 2, (vT + vB) / 2, factor);
  }

  let expanded = $state(false);

  function onKeyDown(e: KeyboardEvent): void {
    if (expanded && e.key === 'Escape') expanded = false;
  }

  // Wheel and Safari gesture events must be non-passive to preventDefault, so
  // wire them by hand. This effect reads plotEl only, never writes it, so it
  // cannot loop.
  $effect(() => {
    const el = plotEl;
    if (el === undefined) return;
    const wheel = onWheel as EventListener;
    const gStart = onGestureStart as unknown as EventListener;
    const gChange = onGestureChange as unknown as EventListener;
    el.addEventListener('wheel', wheel, { passive: false });
    el.addEventListener('gesturestart', gStart, { passive: false });
    el.addEventListener('gesturechange', gChange, { passive: false });
    return () => {
      el.removeEventListener('wheel', wheel);
      el.removeEventListener('gesturestart', gStart);
      el.removeEventListener('gesturechange', gChange);
    };
  });

  const clipId = `plot-clip-${Math.random().toString(36).slice(2)}`;
</script>

<svelte:window onkeydown={onKeyDown} />

<figure class="chart" class:expanded>
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
    <button class="expand" type="button" onclick={() => (expanded = !expanded)}>
      {expanded ? 'Collapse' : 'Expand'}
    </button>
  </div>

  <!-- Direct-manipulation map surface; keyboard users pan/zoom via the controls. -->
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div
    class="plot"
    class:grabbing={dragging}
    bind:this={plotEl}
    onpointerdown={onPointerDown}
    onpointermove={onPointerMove}
    onpointerup={onPointerUp}
    onpointercancel={onPointerUp}
    onclickcapture={onClickCapture}
    ondblclick={onDblClick}
  >
    <svg
      bind:this={svgEl}
      width={W}
      height={H}
      viewBox="0 0 {W} {H}"
      role="img"
      aria-label="Items plotted by {xDim.label} against {yDim.label}; drag to pan, pinch or scroll to zoom"
    >
      <defs>
        <clipPath id={clipId}>
          <rect x={vL - 8} y={0} width={W - vL + 8} height={vB} />
        </clipPath>
      </defs>

      <g clip-path="url(#{clipId})">
        {#each [1, 4, 7, 10] as v (v)}
          <line class="grid" x1={tx(sx(v))} y1={ty(sy(1))} x2={tx(sx(v))} y2={ty(sy(10))} />
          <line class="grid" x1={tx(sx(1))} y1={ty(sy(v))} x2={tx(sx(10))} y2={ty(sy(v))} />
        {/each}

        {#each points as point (point.item.id)}
          <!-- Callers pass resolve()d hrefs; a generic component cannot resolve. -->
          <!-- eslint-disable svelte/no-navigation-without-resolve -->
          <a
            onpointerenter={() => {
              if (!dragging) hovered = point;
            }}
            onpointerleave={() => (hovered = null)}
            href={point.item.href}
          >
            <g class="mark" class:dimmed={hovered !== null && hovered !== point}>
              <circle class="hit" cx={tx(point.px)} cy={ty(point.py)} r="14" />
              <rect
                class="dot"
                x={tx(point.px) - 5}
                y={ty(point.py) - 5}
                width="10"
                height="10"
                style:--c={point.item.colorVar}
              />
              <text class="mark-label" x={tx(point.px) + 10} y={ty(point.py) + 5}>{point.item.label}</text>
            </g>
          </a>
          <!-- eslint-enable svelte/no-navigation-without-resolve -->
        {/each}
      </g>

      <text class="axis-end" x={sx(1)} y={H - 6} text-anchor="start">{xDim.low}</text>
      <text class="axis-end" x={sx(10)} y={H - 6} text-anchor="end">{xDim.high} →</text>
      <text class="axis-end" x={14} y={sy(1)} transform="rotate(-90, 14, {sy(1)})" text-anchor="start">
        {yDim.low}
      </text>
      <text class="axis-end" x={14} y={sy(10)} transform="rotate(-90, 14, {sy(10)})" text-anchor="end">
        {yDim.high} →
      </text>
    </svg>

    <div class="map-controls">
      <button type="button" onclick={() => {
          zoomButton(1.5);
        }} aria-label="Zoom in">+</button>
      <button type="button" onclick={() => {
          zoomButton(1 / 1.5);
        }} aria-label="Zoom out">&minus;</button>
      {#if transformed}
        <button type="button" onclick={reset} aria-label="Reset view">Reset</button>
      {/if}
    </div>

    {#if hovered !== null && !dragging}
      <div
        class="tooltip"
        class:flip={tx(hovered.px) > W * 0.55}
        style:left={`${((tx(hovered.px) / W) * 100).toFixed(2)}%`}
        style:top={`${((ty(hovered.py) / H) * 100).toFixed(2)}%`}
      >
        <strong>{hovered.item.label}</strong> &middot; {hovered.item.title}
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

  .controls .expand {
    margin-left: auto;
    font: inherit;
    height: var(--cell-h);
    color: var(--fg-muted);
    background: var(--bg);
    border: 1px solid var(--rule);
    border-radius: var(--radius);
    padding: 0 1ch;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    cursor: pointer;
  }

  .controls .expand:hover {
    border-color: var(--fg-muted);
    color: var(--fg);
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
    /* Vertical swipes stay the page's scroll; the map takes horizontal
       drags and pinch (its pointer/gesture handlers), so a reader can
       always scroll past the chart. */
    touch-action: pan-y;
    cursor: grab;
  }

  .plot.grabbing {
    cursor: grabbing;
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

  /* Zoom / reset overlay, pinned to the map's top-right like a map's chrome. */
  .map-controls {
    position: absolute;
    top: 1ch;
    right: 1ch;
    display: flex;
    gap: 1ch;
  }

  .map-controls button {
    font: inherit;
    min-width: var(--cell-h);
    height: var(--cell-h);
    display: inline-flex;
    align-items: center;
    justify-content: center;
    color: var(--fg-muted);
    background: var(--bg);
    border: 1px solid var(--rule);
    border-radius: var(--radius);
    padding: 0 1ch;
    cursor: pointer;
  }

  .map-controls button:hover {
    border-color: var(--fg-muted);
    color: var(--fg);
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

  /* Expanded: the map fills the screen; Escape or Collapse exits. */
  .chart.expanded {
    position: fixed;
    inset: 0;
    z-index: 50;
    margin: 0;
    padding: var(--cell-h) 2ch;
    background: var(--bg);
    display: flex;
    flex-direction: column;
  }

  .chart.expanded .plot {
    flex: 1;
    width: auto;
    max-width: none;
    min-height: 0;
    margin-bottom: 0;
  }

  .chart.expanded svg {
    width: 100%;
    height: 100%;
    max-width: none;
  }
</style>
