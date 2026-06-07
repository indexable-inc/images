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
  // A drag on a gutter moves weight between the two panes it sits between; the
  // sizes persist so a refresh keeps the layout the user chose.
  const GUTTER = 6; // px width of a drag handle
  const MIN_PX = 220; // a pane never shrinks below this
  const STORE_KEY = 'ix-mcp-pane-cols';
  const DEFAULT_COLS = [1.4, 1, 0.85];

  function loadCols(): number[] {
    try {
      const raw = localStorage.getItem(STORE_KEY);
      if (raw) {
        const parsed = JSON.parse(raw);
        const valid =
          Array.isArray(parsed) &&
          parsed.length === 3 &&
          parsed.every((n) => typeof n === 'number' && n > 0 && Number.isFinite(n));
        if (valid) return parsed;
      }
    } catch {
      // Unreadable or blocked storage: fall back to the defaults below.
    }
    return [...DEFAULT_COLS];
  }

  let cols = $state(loadCols());
  let panesEl: HTMLDivElement;

  const gridTemplate = $derived(
    `${cols[0]}fr ${GUTTER}px ${cols[1]}fr ${GUTTER}px ${cols[2]}fr`,
  );

  function startDrag(index: number, event: PointerEvent): void {
    // index 0 is the gutter between panes 0 and 1; index 1 between 1 and 2.
    if (!panesEl) return;
    event.preventDefault();
    const handle = event.currentTarget as HTMLElement;
    handle.setPointerCapture(event.pointerId);

    const available = panesEl.clientWidth - 2 * GUTTER;
    const total = cols[0] + cols[1] + cols[2];
    const pxPerFr = available / total;
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
      try {
        localStorage.setItem(STORE_KEY, JSON.stringify(cols));
      } catch {
        // Storage may be blocked; the live layout still holds for this session.
      }
    }
    handle.addEventListener('pointermove', onMove);
    handle.addEventListener('pointerup', onUp);
  }

  function resetCols(): void {
    cols = [...DEFAULT_COLS];
    try {
      localStorage.removeItem(STORE_KEY);
    } catch {
      // Nothing to clear if storage is unavailable.
    }
  }

  // Each column owns its own scroll, so the page never scrolls as a whole and a
  // refresh to one column never moves another. Stick-to-bottom on executions only
  // re-pins when the user was already near the bottom; scrolling up frees it.
  let execBody: HTMLDivElement;
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

<header class="top">
  <span class="brand"><b>ix</b> &middot; mcp</span>
  <span class="spacer"></span>
  <span class="stat" class:stale={!feed.connected}>
    {#if running}<span class="dot"></span><b>{running}</b> running &nbsp;{/if}
    <b>{feed.jobs.length}</b> runs
  </span>
</header>

<div class="panes" bind:this={panesEl} style="grid-template-columns: {gridTemplate}">
  <!-- The agent's curated highlight reel: the most important results, presented. -->
  <section class="pane cells-pane">
    <div class="sec">cells <span class="count">{feed.cells.length}</span></div>
    <div class="pane-body">
      {#if feed.cells.length === 0}
        <div class="empty">the agent has not pinned any results yet</div>
      {:else}
        {#each feed.cells as cell (cell.id)}
          <CellCard {cell} />
        {/each}
      {/if}
    </div>
  </section>

  <!-- Drag to resize the panes either side; double-click to reset. -->
  <div
    class="gutter"
    role="separator"
    aria-orientation="vertical"
    aria-label="Resize cells and executions"
    tabindex="-1"
    onpointerdown={(e) => startDrag(0, e)}
    ondblclick={resetCols}
  ></div>

  <!-- Every run, oldest first, streaming live as it goes. -->
  <section class="pane exec-pane">
    <div class="sec">executions <span class="count">{feed.jobs.length}</span></div>
    <div class="pane-body" bind:this={execBody} onscroll={trackExec}>
      {#if ordered.length === 0}
        <div class="empty">no executions yet</div>
      {:else}
        {#each ordered as job (job.id)}
          <JobCard {job} />
        {/each}
      {/if}
    </div>
  </section>

  <div
    class="gutter"
    role="separator"
    aria-orientation="vertical"
    aria-label="Resize executions and resources"
    tabindex="-1"
    onpointerdown={(e) => startDrag(1, e)}
    ondblclick={resetCols}
  ></div>

  <!-- Live, self-updating views: a terminal screen, a VM framebuffer, a widget. -->
  <section class="pane res-pane">
    <div class="sec">resources <span class="count">{feed.resources.length}</span></div>
    <div class="pane-body">
      {#if feed.resources.length === 0}
        <div class="empty">no live resources</div>
      {:else}
        {#each feed.resources as resource (resource.id)}
          <ResourceCard {resource} />
        {/each}
      {/if}
    </div>
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
     target past the 6px track without changing the layout. */
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
  .pane-body {
    flex: 1 1 auto;
    min-height: 0;
    overflow: auto;
    overflow-anchor: none;
    padding: 14px 16px 32px;
  }
  .sec {
    flex: none;
    display: flex;
    align-items: center;
    gap: 8px;
    margin: 0;
    padding: 9px 16px;
    color: var(--muted);
    font-size: 10px;
    font-weight: 600;
    letter-spacing: 0.2em;
    text-transform: uppercase;
    border-bottom: 1px solid var(--line);
    background: var(--bg);
  }
  .sec .count {
    color: var(--faint);
    letter-spacing: 0.04em;
  }
  .empty {
    padding: 2px 0;
    color: var(--faint);
    font-size: 12px;
    font-style: italic;
  }

  /* Stack the columns on a narrow screen; the page scrolls and each pane sizes
     to its content rather than competing for one screen's height. The grid and
     its drag gutters give way to a simple vertical flow. */
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
  }
</style>
