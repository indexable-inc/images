<script lang="ts">
  import { feed } from '$lib/feed.svelte';
  import { now } from '$lib/now.svelte';
  import JobCard from '$components/JobCard.svelte';
  import CellCard from '$components/CellCard.svelte';
  import ResourceCard from '$components/ResourceCard.svelte';

  // Executions read like a notebook: oldest at top, newest at the bottom.
  const ordered = $derived([...feed.jobs].sort((a, b) => a.started_at - b.started_at));
  const running = $derived(feed.jobs.filter((j) => j.status === 'running').length);

  // The three panes are grid tracks sized in `fr`, so they always fill the row.
  // A drag on a gutter moves weight between the two panes it sits between. Each
  // pane header is a toggle: click it to collapse the pane to a thin labelled
  // strip, freeing its width for the panes that stay open. Cells and executions
  // are the work; resources are a secondary view, so they start collapsed. Both
  // the fr weights and the collapsed flags persist, so a refresh keeps the
  // layout the user chose.
  const GUTTER = 6; // px width of a live drag handle
  const HAIRLINE = 1; // px width of the seam beside a collapsed pane
  const COLLAPSED_PX = 38; // width of a collapsed pane's labelled strip
  const MIN_PX = 220; // an open pane never shrinks below this
  const DEFAULT_COLS = [1.4, 1, 0.85];
  const DEFAULT_COLLAPSED = [false, false, true]; // resources start folded away

  const COLS_KEY = 'ix-mcp-pane-cols';
  const COLLAPSED_KEY = 'ix-mcp-pane-collapsed';

  // One small loader for both bits of persisted state: read JSON, accept it only
  // when it has the right shape, otherwise fall back to the supplied default.
  function load<T>(key: string, fallback: T, valid: (v: unknown) => boolean): T {
    try {
      const raw = localStorage.getItem(key);
      if (raw !== null) {
        const parsed = JSON.parse(raw);
        if (valid(parsed)) return parsed as T;
      }
    } catch {
      // Unreadable or blocked storage: fall back to the default below.
    }
    return fallback;
  }

  function save(key: string, value: unknown): void {
    try {
      localStorage.setItem(key, JSON.stringify(value));
    } catch {
      // Storage may be blocked; the live layout still holds for this session.
    }
  }

  const isTriple = (pred: (n: unknown) => boolean) => (v: unknown) =>
    Array.isArray(v) && v.length === 3 && v.every(pred);

  let cols = $state(
    load(COLS_KEY, [...DEFAULT_COLS], isTriple((n) => typeof n === 'number' && n > 0 && Number.isFinite(n))),
  );
  let collapsed = $state(
    load(COLLAPSED_KEY, [...DEFAULT_COLLAPSED], isTriple((b) => typeof b === 'boolean')),
  );
  let panesEl: HTMLDivElement;

  // A collapsed pane is a fixed strip; an open one takes its fr share. A gutter
  // is only a live drag handle between two open panes; beside a collapsed pane
  // it shrinks to a hairline seam.
  const track = (i: number) => (collapsed[i] ? `${COLLAPSED_PX}px` : `${cols[i]}fr`);
  const seam = (g: number) => (collapsed[g] || collapsed[g + 1] ? `${HAIRLINE}px` : `${GUTTER}px`);
  const gridTemplate = $derived(
    `${track(0)} ${seam(0)} ${track(1)} ${seam(1)} ${track(2)}`,
  );

  function toggle(index: number): void {
    const next = [...collapsed];
    next[index] = !next[index];
    collapsed = next;
    save(COLLAPSED_KEY, collapsed);
  }

  function startDrag(index: number, event: PointerEvent): void {
    // index 0 is the gutter between panes 0 and 1; index 1 between 1 and 2. A
    // gutter beside a collapsed pane is inert: there is no weight to move.
    if (!panesEl || collapsed[index] || collapsed[index + 1]) return;
    event.preventDefault();
    const handle = event.currentTarget as HTMLElement;
    handle.setPointerCapture(event.pointerId);

    // Convert pixels to fr against the two panes actually being resized, so the
    // maths stays correct no matter what the third pane is doing.
    const paneEls = panesEl.querySelectorAll<HTMLElement>('.pane');
    const combinedPx = paneEls[index].clientWidth + paneEls[index + 1].clientWidth;
    const sumFr = cols[index] + cols[index + 1];
    const pxPerFr = combinedPx / sumFr;
    const minFr = MIN_PX / pxPerFr;
    const startX = event.clientX;
    const startA = cols[index];
    const startB = cols[index + 1];

    function onMove(e: PointerEvent): void {
      let deltaFr = (e.clientX - startX) / pxPerFr;
      // Clamp so neither neighbour drops below the minimum width.
      deltaFr = Math.max(minFr - startA, Math.min(startB - minFr, deltaFr));
      const next = [...cols];
      next[index] = startA + deltaFr;
      next[index + 1] = startB - deltaFr;
      cols = next;
    }
    function onUp(e: PointerEvent): void {
      handle.releasePointerCapture(e.pointerId);
      handle.removeEventListener('pointermove', onMove);
      handle.removeEventListener('pointerup', onUp);
      save(COLS_KEY, cols);
    }
    handle.addEventListener('pointermove', onMove);
    handle.addEventListener('pointerup', onUp);
  }

  function resetLayout(): void {
    cols = [...DEFAULT_COLS];
    collapsed = [...DEFAULT_COLLAPSED];
    save(COLS_KEY, cols);
    save(COLLAPSED_KEY, collapsed);
  }

  // Each column owns its own scroll, so the page never scrolls as a whole and a
  // refresh to one column never moves another. Stick-to-bottom on executions only
  // re-pins when the user was already near the bottom; scrolling up frees it.
  let execBody: HTMLDivElement | undefined;
  let pinned = true;
  function trackExec(): void {
    if (!execBody) return;
    pinned = execBody.scrollHeight - execBody.scrollTop - execBody.clientHeight < 80;
  }

  $effect(() => {
    feed.start();
    now.start();
    return () => {
      feed.stop();
      now.stop();
    };
  });

  $effect(() => {
    // Re-pin to the bottom after an executions update, but only if the user was
    // already there. Depend on the array so this runs on each real change.
    void feed.jobs;
    if (pinned && execBody) {
      requestAnimationFrame(() => {
        if (execBody) execBody.scrollTop = execBody.scrollHeight;
      });
    }
  });
</script>

<!-- A pane header doubles as its collapse toggle: the caret shows the next
     action and the strip stays clickable when the pane is folded. -->
{#snippet head(index: number, label: string, count: number)}
  <button
    class="sec"
    type="button"
    aria-expanded={!collapsed[index]}
    title={collapsed[index] ? `Show ${label}` : `Hide ${label}`}
    onclick={() => toggle(index)}
  >
    <span class="caret" aria-hidden="true">{collapsed[index] ? '▸' : '▾'}</span>
    <span class="label">{label}</span>
    <span class="count">{count}</span>
  </button>
{/snippet}

<header class="top">
  <span class="brand"><b>ix</b> &middot; mcp</span>
  <span class="spacer"></span>
  <button class="reset" type="button" title="Reset layout" onclick={resetLayout}>reset</button>
  <span class="stat" class:stale={!feed.connected}>
    {#if running}<span class="dot"></span><b>{running}</b> running &nbsp;{/if}
    <b>{feed.jobs.length}</b> runs
  </span>
</header>

<div class="panes" bind:this={panesEl} style="grid-template-columns: {gridTemplate}">
  <!-- The agent's curated highlight reel: the most important results, presented. -->
  <section class="pane cells-pane" class:collapsed={collapsed[0]}>
    {@render head(0, 'cells', feed.cells.length)}
    {#if !collapsed[0]}
      <div class="pane-body">
        {#if feed.cells.length === 0}
          <div class="empty">the agent has not pinned any results yet</div>
        {:else}
          {#each feed.cells as cell (cell.id)}
            <CellCard {cell} />
          {/each}
        {/if}
      </div>
    {/if}
  </section>

  <!-- Drag to resize the panes either side; double-click to reset. -->
  <div
    class="gutter"
    class:inert={collapsed[0] || collapsed[1]}
    role="separator"
    aria-orientation="vertical"
    aria-label="Resize cells and executions"
    tabindex="-1"
    onpointerdown={(e) => startDrag(0, e)}
    ondblclick={resetLayout}
  ></div>

  <!-- Every run, oldest first, streaming live as it goes. -->
  <section class="pane exec-pane" class:collapsed={collapsed[1]}>
    {@render head(1, 'executions', feed.jobs.length)}
    {#if !collapsed[1]}
      <div class="pane-body" bind:this={execBody} onscroll={trackExec}>
        {#if ordered.length === 0}
          <div class="empty">no executions yet</div>
        {:else}
          {#each ordered as job (job.id)}
            <JobCard {job} />
          {/each}
        {/if}
      </div>
    {/if}
  </section>

  <div
    class="gutter"
    class:inert={collapsed[1] || collapsed[2]}
    role="separator"
    aria-orientation="vertical"
    aria-label="Resize executions and resources"
    tabindex="-1"
    onpointerdown={(e) => startDrag(1, e)}
    ondblclick={resetLayout}
  ></div>

  <!-- Live, self-updating views: a terminal screen, a VM framebuffer, a widget. -->
  <section class="pane res-pane" class:collapsed={collapsed[2]}>
    {@render head(2, 'resources', feed.resources.length)}
    {#if !collapsed[2]}
      <div class="pane-body">
        {#if feed.resources.length === 0}
          <div class="empty">no live resources</div>
        {:else}
          {#each feed.resources as resource (resource.id)}
            <ResourceCard {resource} />
          {/each}
        {/if}
      </div>
    {/if}
  </section>
</div>

<style>
  .top {
    flex: none;
    display: flex;
    gap: 12px;
    align-items: center;
    padding: 11px 18px;
    background: rgba(11, 11, 12, 0.86);
    backdrop-filter: blur(8px);
    border-bottom: 1px solid var(--line);
  }
  .brand {
    color: var(--dim);
    font-size: 11px;
    font-weight: 600;
    letter-spacing: 0.22em;
    text-transform: uppercase;
  }
  .brand :global(b) {
    color: var(--text);
    font-weight: 600;
  }
  .spacer {
    flex: 1;
  }
  .reset {
    appearance: none;
    border: 1px solid var(--line);
    background: transparent;
    color: var(--muted);
    font: inherit;
    font-size: 10px;
    letter-spacing: 0.18em;
    text-transform: uppercase;
    padding: 4px 9px;
    cursor: pointer;
  }
  .reset:hover {
    color: var(--text);
    border-color: var(--active);
  }
  .stat {
    color: var(--muted);
    font-size: 11px;
    letter-spacing: 0.04em;
  }
  .stat :global(b) {
    color: var(--text);
    font-weight: 600;
  }
  .stat.stale {
    color: var(--err);
  }
  .dot {
    display: inline-block;
    width: 6px;
    height: 6px;
    margin-right: 6px;
    background: var(--active);
    vertical-align: middle;
  }

  /* Three columns filling the viewport; each scrolls on its own. The track
     sizes come from `gridTemplate` (inline), with drag gutters between them. */
  .panes {
    flex: 1 1 auto;
    min-height: 0;
    display: grid;
    background: var(--bg);
  }
  .pane {
    display: flex;
    flex-direction: column;
    min-width: 0;
    min-height: 0;
    background: var(--bg);
  }

  /* A thin separator that drags horizontally. The ::after widens the pointer
     target past the 6px track without changing the layout. Beside a collapsed
     pane the gutter is inert: it carries no weight, so it reads as a plain seam. */
  .gutter {
    background: var(--line);
    cursor: col-resize;
    position: relative;
    touch-action: none;
  }
  .gutter::after {
    content: '';
    position: absolute;
    inset: 0 -4px;
  }
  .gutter:hover,
  .gutter:active {
    background: var(--active);
  }
  .gutter.inert {
    cursor: default;
  }
  .gutter.inert::after {
    display: none;
  }
  .gutter.inert:hover,
  .gutter.inert:active {
    background: var(--line);
  }
  .pane-body {
    flex: 1 1 auto;
    min-height: 0;
    overflow: auto;
    overflow-anchor: none;
    padding: 14px 16px 32px;
  }

  /* The header is the collapse toggle, so it is a full-width button styled to
     read as a section label. */
  .sec {
    flex: none;
    appearance: none;
    width: 100%;
    display: flex;
    align-items: center;
    gap: 8px;
    margin: 0;
    padding: 9px 16px;
    border: 0;
    border-bottom: 1px solid var(--line);
    background: var(--bg);
    color: var(--muted);
    font: inherit;
    font-size: 10px;
    font-weight: 600;
    letter-spacing: 0.2em;
    text-transform: uppercase;
    text-align: left;
    cursor: pointer;
  }
  .sec:hover {
    color: var(--text);
  }
  .sec:focus-visible {
    outline: 1px solid var(--active);
    outline-offset: -1px;
  }
  .caret {
    color: var(--faint);
    font-size: 9px;
    line-height: 1;
  }
  .sec:hover .caret {
    color: var(--active);
  }
  .count {
    color: var(--faint);
    letter-spacing: 0.04em;
  }

  /* Collapsed: the strip turns its header on its side so the label reads down
     the column. The caret stays upright as a clear affordance. */
  .pane.collapsed {
    overflow: hidden;
  }
  .pane.collapsed .sec {
    writing-mode: vertical-rl;
    height: 100%;
    justify-content: flex-start;
    gap: 12px;
    padding: 14px 0;
    border-bottom: 0;
    border-left: 1px solid var(--line);
  }
  .pane.collapsed .caret {
    writing-mode: horizontal-tb;
  }

  .empty {
    padding: 2px 0;
    color: var(--faint);
    font-size: 12px;
    font-style: italic;
  }

  /* Stack the columns on a narrow screen; the page scrolls and each pane sizes
     to its content rather than competing for one screen's height. The grid and
     its drag gutters give way to a simple vertical flow, and a collapsed pane is
     just its horizontal header. */
  @media (max-width: 1000px) {
    .panes {
      display: flex;
      flex-direction: column;
      overflow: auto;
    }
    .pane {
      flex: none;
    }
    .gutter {
      display: none;
    }
    .pane-body {
      overflow: visible;
    }
    .pane.collapsed .sec {
      writing-mode: horizontal-tb;
      height: auto;
      padding: 9px 16px;
      border-bottom: 1px solid var(--line);
      border-left: 0;
    }
  }
</style>
