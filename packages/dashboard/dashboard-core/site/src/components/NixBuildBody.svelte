<script lang="ts">
  // The `nix-build` data renderer: a live Nix build tree. The pane's `body` is a
  // JSON `BuildView` (owned by the Rust nix-web-monitor parser crate and streamed
  // by its `--emit ndjson` mode; see packages/nix/nix-web-monitor). This draws it
  // compactly and still — a summary bar of status counts, the derivation rows with
  // phase and a status dot, an in-flight-activity strip with progress bars, and any
  // errors highlighted. Colors are theme variables, so it follows the dashboard's
  // light/dark scheme with no per-mode markup.
  import type { Pane } from '$lib/types';

  let { pane }: { pane: Pane } = $props();

  type BuildStatus = 'planned' | 'running' | 'stopped' | 'succeeded' | 'failed';

  interface BuildRow {
    derivation: string;
    name: string;
    status: BuildStatus;
    phase?: string | null;
    host?: string | null;
    logCount: number;
    contentAddressed: boolean;
  }

  interface ActivityRow {
    kind: string;
    text: string;
    done: number;
    expected: number;
    sizeBytes?: number | null;
  }

  interface Counts {
    planned: number;
    running: number;
    stopped: number;
    succeeded: number;
    failed: number;
  }

  interface BuildView {
    command: string;
    builds: BuildRow[];
    activities: ActivityRow[];
    counts: Counts;
    errors: string[];
    finished: boolean;
    exitCode?: number | null;
  }

  const view = $derived.by<BuildView | null>(() => {
    try {
      const parsed: unknown = JSON.parse(pane.body ?? '');
      return parsed && typeof parsed === 'object' ? (parsed as BuildView) : null;
    } catch {
      return null;
    }
  });

  const counts = $derived(
    view?.counts ?? { planned: 0, running: 0, stopped: 0, succeeded: 0, failed: 0 },
  );
  // Sort rows so the eye lands on what matters: failures first, then running,
  // then the rest in first-seen order. A stable index keeps equal-rank rows put.
  const rank: Record<BuildStatus, number> = {
    failed: 0,
    running: 1,
    stopped: 2,
    planned: 3,
    succeeded: 4,
  };
  const builds = $derived.by(() =>
    (view?.builds ?? [])
      .map((b, i) => ({ b, i }))
      .sort((x, y) => rank[x.b.status] - rank[y.b.status] || x.i - y.i)
      .map(({ b }) => b),
  );
  const activities = $derived(view?.activities ?? []);
  const errors = $derived(view?.errors ?? []);

  // The status chips shown in the summary bar, in a fixed order; zero-count ones
  // are dropped so the bar stays quiet on a small build.
  const chips = $derived(
    (
      [
        ['failed', counts.failed],
        ['running', counts.running],
        ['succeeded', counts.succeeded],
        ['planned', counts.planned],
        ['stopped', counts.stopped],
      ] as [BuildStatus, number][]
    ).filter(([, n]) => n > 0),
  );

  const statusMark: Record<BuildStatus, string> = {
    planned: '○',
    running: '▶',
    stopped: '◼',
    succeeded: '✓',
    failed: '✗',
  };

  const state = $derived(
    view?.finished ? (errors.length || view.exitCode ? 'failed' : 'done') : 'building',
  );

  function pct(done: number, expected: number): number | null {
    if (!expected || expected <= 0) return null;
    return Math.max(0, Math.min(100, Math.round((done / expected) * 100)));
  }

  function fmtBytes(n: number | null | undefined): string {
    if (n == null || n <= 0) return '';
    const units = ['B', 'KB', 'MB', 'GB', 'TB'];
    let v = n;
    let u = 0;
    while (v >= 1024 && u < units.length - 1) {
      v /= 1024;
      u += 1;
    }
    return `${v < 10 && u > 0 ? v.toFixed(1) : Math.round(v)} ${units[u]}`;
  }
</script>

<div class="nb">
  {#if !view}
    <div class="nb-empty">starting…</div>
  {:else}
    <header class="nb-head">
      <span class="nb-dot nb-{state}"></span>
      <span class="nb-cmd" title={view.command}>{view.command}</span>
      <span class="nb-chips">
        {#each chips as [status, n] (status)}
          <span class="nb-chip nb-{status}">{statusMark[status]} {n}</span>
        {/each}
      </span>
    </header>

    {#if errors.length}
      <div class="nb-errors">
        {#each errors as err, i (i)}
          <pre class="nb-err">{err}</pre>
        {/each}
      </div>
    {/if}

    {#if builds.length}
      <ul class="nb-list">
        {#each builds as b (b.derivation)}
          <li class="nb-row">
            <span class="nb-mark nb-{b.status}">{statusMark[b.status]}</span>
            <span class="nb-name">{b.name}</span>
            {#if b.phase}<span class="nb-phase">{b.phase}</span>{/if}
            {#if b.contentAddressed}<span class="nb-tag">ca</span>{/if}
            {#if b.host}<span class="nb-host">{b.host}</span>{/if}
          </li>
        {/each}
      </ul>
    {/if}

    {#if activities.length}
      <ul class="nb-acts">
        {#each activities as a, i (i)}
          {@const p = pct(a.done, a.expected)}
          <li class="nb-act">
            <span class="nb-akind">{a.kind}</span>
            <span class="nb-atext" title={a.text}>{a.text}</span>
            {#if p !== null}
              <span class="nb-bar"><span class="nb-fill" style="width:{p}%"></span></span>
            {:else if a.sizeBytes}
              <span class="nb-size">{fmtBytes(a.sizeBytes)}</span>
            {/if}
          </li>
        {/each}
      </ul>
    {/if}

    {#if !builds.length && !activities.length && !errors.length}
      <div class="nb-empty">no derivations yet</div>
    {/if}
  {/if}
</div>

<style>
  .nb {
    font-family: var(--mono);
    font-size: 12px;
    padding: 6px 0 8px;
    color: var(--ink);
  }
  .nb-empty {
    padding: 10px 14px;
    color: var(--ink-faint);
    font-style: italic;
  }
  .nb-head {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 6px 12px 8px;
    border-bottom: 1px solid var(--edge);
  }
  .nb-cmd {
    color: var(--ink);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    min-width: 0;
    flex: 1 1 auto;
  }
  .nb-chips {
    display: flex;
    gap: 6px;
    flex: none;
  }
  .nb-chip {
    font-size: 11px;
    font-variant-numeric: tabular-nums;
    color: var(--ink-dim);
  }
  /* A small status dot for the whole run, and per-status colors reused by the
     row marks and chips. Semantic colors are hard-coded (they carry meaning that
     must survive both themes); everything else is a theme variable. */
  .nb-dot {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    flex: none;
    background: var(--ink-faint);
  }
  .nb-dot.nb-building {
    background: #4b8bf5;
  }
  .nb-dot.nb-done {
    background: #3fb950;
  }
  .nb-dot.nb-failed {
    background: #e5534b;
  }
  .nb-failed {
    color: #e5534b;
  }
  .nb-running {
    color: #4b8bf5;
  }
  .nb-succeeded {
    color: #3fb950;
  }
  .nb-planned,
  .nb-stopped {
    color: var(--ink-faint);
  }
  .nb-errors {
    padding: 6px 12px;
    border-bottom: 1px solid var(--edge);
  }
  .nb-err {
    margin: 0;
    white-space: pre-wrap;
    word-break: break-word;
    color: #e5534b;
    font-size: 11.5px;
    line-height: 1.4;
  }
  .nb-list,
  .nb-acts {
    list-style: none;
    margin: 0;
    padding: 4px 0;
  }
  .nb-acts {
    border-top: 1px solid var(--edge);
  }
  .nb-row,
  .nb-act {
    display: flex;
    align-items: baseline;
    gap: 8px;
    padding: 2px 12px;
    line-height: 1.5;
  }
  .nb-mark {
    flex: none;
    width: 1ch;
    text-align: center;
  }
  .nb-name {
    color: var(--ink);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    min-width: 0;
  }
  .nb-phase {
    flex: none;
    color: var(--ink-dim);
    font-size: 11px;
  }
  .nb-tag {
    flex: none;
    font-size: 10px;
    color: var(--ink-faint);
    border: 1px solid var(--edge);
    border-radius: 3px;
    padding: 0 3px;
  }
  .nb-host {
    flex: none;
    margin-left: auto;
    color: var(--ink-faint);
    font-size: 11px;
  }
  .nb-akind {
    flex: none;
    color: var(--ink-dim);
    font-size: 11px;
    min-width: 7ch;
  }
  .nb-atext {
    color: var(--ink-faint);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    min-width: 0;
    flex: 1 1 auto;
    font-size: 11px;
  }
  .nb-bar {
    flex: none;
    width: 60px;
    height: 5px;
    border-radius: 3px;
    background: var(--edge);
    overflow: hidden;
  }
  .nb-fill {
    display: block;
    height: 100%;
    background: #4b8bf5;
  }
  .nb-size {
    flex: none;
    color: var(--ink-faint);
    font-size: 11px;
    font-variant-numeric: tabular-nums;
  }
</style>
