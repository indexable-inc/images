<script lang="ts">
  // The unified left sidebar: the ix logotype + live dot, a filter box, then three
  // foldable sections — SESSIONS (each session foldable, its runs nested
  // oldest-first, a log growing downward), RESOURCES, and RECORDINGS. It owns the center-stage selection
  // and the vim keyboard nav; fold state is persisted in ui. Folding is driven by
  // ui.folds (not native <details>) so the flattened keyboard walk matches exactly
  // what is visible.
  import { onMount } from 'svelte';
  import { store, timeline, loadRecording } from '$lib/stream.svelte';
  import { ui, isOpen, toggleFold, setFold, select, focusPane, humanTime, humanAge, humanClock, runTooltip } from '$lib/ui.svelte';
  import { setListNav } from '$lib/keys.svelte';
  import {
    buildSidebar,
    flattenVisible,
    resourceMeta,
    selectionEq,
    type Selection,
  } from '$lib/sidebar';
  import { kindOf, paneId } from '$lib/run';

  const model = $derived(buildSidebar(store.panes, timeline.recordings));

  const refMs = $derived(
    timeline.source === 'live' && timeline.following ? ui.clock : timeline.position || timeline.maxTs,
  );

  // `/` filter: a case-insensitive substring match over a run's title, subtitle,
  // pane id, and lang (matching the old feed's reach), so a long tree narrows.
  // Empty query shows everything. A session shows if it or any of its runs match;
  // a run shows if it or its session matches.
  let query = $state('');
  let filterEl: HTMLInputElement | undefined;
  const q = $derived(query.trim().toLowerCase());

  function runMatches(r: typeof model.sessions[number]['runs'][number]): boolean {
    if (!q) return true;
    return [r.pane.title, r.pane.subtitle, r.pane.lang, paneId(r.key)].some((v) =>
      (v ?? '').toLowerCase().includes(q),
    );
  }
  // A session's visible runs under the filter: all if the session label matches,
  // else only the matching runs.
  function visibleRuns(label: string, runs: typeof model.sessions[number]['runs']) {
    if (!q || label.toLowerCase().includes(q)) return runs;
    return runs.filter(runMatches);
  }

  const sessions = $derived(
    model.sessions
      .map((s) => ({ ...s, runs: visibleRuns(s.label, s.runs) }))
      .filter((s) => s.runs.length > 0),
  );
  const resources = $derived(
    model.resources.filter(runMatches),
  );
  const recordings = $derived(
    model.recordings.filter((rec) => !q || rec.id.toLowerCase().includes(q)),
  );

  // The filtered model the keyboard walks. Fold checks read ui via isOpen.
  const filteredModel = $derived({ ...model, sessions, resources, recordings });
  const flat = $derived(flattenVisible(filteredModel, isOpen));

  // ----- selection -------------------------------------------------------
  function onSelect(sel: Selection): void {
    select(sel);
    if (sel.kind === 'recording') {
      // Mirror the status-bar picker: once the recording's panes are in, land on
      // its first run so the center stage shows content, not the empty prompt.
      // If the recording opens with no runs at its start, keep the recording
      // selected — the prompt to scrub is then accurate.
      void loadRecording(sel.id).then(() => {
        const first = flat.find((f) => f.selection.kind !== 'recording');
        if (first) select(first.selection);
      });
    }
  }

  // Keep the selection valid as the tree changes; default to the newest visible
  // run (the runs are log-ordered, so that is the LAST run row), else the first
  // resource, so a fresh load shows something current. Never repair onto a
  // recording row: onSelect would loadRecording() and close the live stream
  // without a user click (e.g. when /recordings wins the race against the
  // first SSE snapshot, or when filtering folds every run away).
  $effect(() => {
    if (flat.length === 0) {
      if (ui.selection) select(null);
      return;
    }
    if (!flat.some((f) => selectionEq(f.selection, ui.selection))) {
      // Newest by timestamp, not render order: runs are oldest-first within a
      // session, and a filter/fold can hide the globally newest run, so the
      // first visible row isn't necessarily the most recent one.
      const runs = flat.filter((f) => f.selection.kind === 'run');
      const newest = runs.reduce<(typeof runs)[number] | null>((best, f) => {
        const key = (f.selection as { key: string }).key;
        const t = store.panes[key]?.created_at ?? 0;
        const bt = best ? (store.panes[(best.selection as { key: string }).key]?.created_at ?? 0) : -1;
        return t >= bt ? f : best;
      }, null);
      const fallback = newest ?? flat.find((f) => f.selection.kind === 'resource');
      if (fallback) onSelect(fallback.selection);
      else if (ui.selection) select(null);
    }
  });

  function scrollSelected(): void {
    const sel = ui.selection;
    if (!sel) return;
    const key = sel.kind === 'recording' ? 'rec:' + sel.id : sel.key;
    queueMicrotask(() =>
      document
        .querySelector(`[data-nav="${CSS.escape(key)}"]`)
        ?.scrollIntoView({ block: 'nearest' }),
    );
  }
  function selectIndex(i: number): void {
    if (!flat.length) return;
    const n = Math.max(0, Math.min(flat.length - 1, i));
    onSelect(flat[n].selection);
    scrollSelected();
  }
  function move(delta: number): void {
    const i = flat.findIndex((f) => selectionEq(f.selection, ui.selection));
    selectIndex((i < 0 ? 0 : i) + delta);
  }

  // The fold key of the selection's owning session (runs live under one), for
  // h/za to act on the enclosing session.
  function sessionFoldKeyOf(sel: Selection | null): string | null {
    if (!sel || sel.kind !== 'run') return null;
    for (const s of sessions) if (s.runs.some((r) => r.key === sel.key)) return 'sess:' + s.scope;
    return null;
  }

  // `o`/Enter/`l`: open a resource or a rich-output attachment fullscreen; for a
  // run, opening its rich output if present, else no-op (the detail is already
  // shown in the center).
  function open(): void {
    const sel = ui.selection;
    if (!sel) return;
    if (sel.kind === 'resource') {
      focusPane(sel.key);
    } else if (sel.kind === 'run') {
      const out = sel.key + '/out';
      if (store.panes[out]) focusPane(out);
    }
  }
  // `h`: fold the enclosing session (if a run is selected), else fold the section.
  function back(): void {
    const foldKey = sessionFoldKeyOf(ui.selection);
    if (foldKey) setFold(foldKey, false);
  }
  // `za`: toggle the enclosing session's fold.
  function fold(): void {
    const foldKey = sessionFoldKeyOf(ui.selection);
    if (foldKey) toggleFold(foldKey);
  }

  onMount(() => {
    setListNav({
      move,
      top: () => selectIndex(0),
      bottom: () => selectIndex(flat.length - 1),
      open,
      back,
      fold,
      filter: () => filterEl?.focus(),
    });
    return () => setListNav(null);
  });
</script>

<nav class="sidebar">
  <div class="sidebar-brand">
    <span class="logo">ix</span>
    <span class="brand-dot" class:live={store.live} title={store.live ? 'connected' : store.status}></span>
  </div>
  <div class="sidebar-filter">
    <input
      type="text"
      placeholder="filter runs, resources…"
      bind:value={query}
      bind:this={filterEl}
      spellcheck="false"
      autocomplete="off"
      aria-label="filter"
    />
  </div>

  <div class="sidebar-scroll">
    <!-- SESSIONS -->
    <section class="nav-section">
      <button class="section-head" onclick={() => toggleFold('sessions')} aria-expanded={isOpen('sessions')}>
        <span class="caret" class:open={isOpen('sessions')}></span>
        <span class="section-name">sessions</span>
        <span class="count">{model.sessions.length}</span>
      </button>
      {#if isOpen('sessions')}
        {#if sessions.length === 0}
          <div class="section-empty">{store.live ? 'no runs yet' : 'connecting…'}</div>
        {/if}
        {#each sessions as s (s.scope)}
          {@const foldKey = 'sess:' + s.scope}
          {@const age = humanAge(s.lastActivity, refMs)}
          <button
            class="session-head"
            onclick={() => toggleFold(foldKey)}
            aria-expanded={isOpen(foldKey)}
            title={s.label}
          >
            <span class="caret" class:open={isOpen(foldKey)}></span>
            <span class="session-name">{s.label}</span>
            <span class="session-age">{age ? `active ${age}` : ''}</span>
          </button>
          {#if isOpen(foldKey)}
            {#each s.runs as r (r.key)}
              {@const running = r.led === 'running'}
              <button
                class="run-row"
                class:selected={ui.selection?.kind === 'run' && ui.selection.key === r.key}
                data-nav={r.key}
                onclick={() => onSelect({ kind: 'run', key: r.key })}
                title={runTooltip(running, r.pane.duration_ms, r.pane.created_at, refMs) || r.pane.title || ''}
              >
                <span class="led led-{r.led}"></span>
                <span class="run-intent">{r.pane.title || '(run)'}</span>
                <span class="run-time">{humanTime(r.pane.created_at, refMs)}</span>
              </button>
            {/each}
          {/if}
        {/each}
      {/if}
    </section>

    <!-- RESOURCES -->
    <section class="nav-section">
      <button class="section-head" onclick={() => toggleFold('resources')} aria-expanded={isOpen('resources')}>
        <span class="caret" class:open={isOpen('resources')}></span>
        <span class="section-name">resources</span>
        <span class="count">{model.resources.length}</span>
      </button>
      {#if isOpen('resources')}
        {#if resources.length === 0}
          <div class="section-empty">none</div>
        {/if}
        {#each resources as r (r.key)}
          <button
            class="res-row"
            class:selected={ui.selection?.kind === 'resource' && ui.selection.key === r.key}
            data-nav={r.key}
            onclick={() => onSelect({ kind: 'resource', key: r.key })}
            title={r.pane.title || ''}
          >
            <span class="res-icon">{kindOf(r.pane) === 'terminal' ? '▮' : '▤'}</span>
            <span class="res-name">{r.pane.title || '(resource)'}</span>
            <span class="led led-{r.led}"></span>
            <span class="res-meta">{resourceMeta(r.pane)}</span>
          </button>
        {/each}
      {/if}
    </section>

    <!-- RECORDINGS -->
    <section class="nav-section">
      <button class="section-head" onclick={() => toggleFold('recordings')} aria-expanded={isOpen('recordings')}>
        <span class="caret" class:open={isOpen('recordings')}></span>
        <span class="section-name">recordings</span>
        <span class="count">{model.recordings.length}</span>
      </button>
      {#if isOpen('recordings')}
        {#if recordings.length === 0}
          <div class="section-empty">none</div>
        {/if}
        {#each recordings as rec (rec.id)}
          <button
            class="rec-row"
            class:selected={ui.selection?.kind === 'recording' && ui.selection.id === rec.id}
            data-nav={'rec:' + rec.id}
            onclick={() => onSelect({ kind: 'recording', id: rec.id })}
            title={rec.id}
          >
            <span class="res-icon">●</span>
            <span class="res-name">{humanClock(rec.started_ms)}</span>
            <span class="res-meta">{(rec.bytes / 1024).toFixed(0)}kb</span>
          </button>
        {/each}
      {/if}
    </section>
  </div>
</nav>

<style>
  .sidebar {
    width: 260px;
    flex: none;
    background: var(--panel);
    border-right: 1px solid var(--edge);
    display: flex;
    flex-direction: column;
    min-height: 0;
  }
  .sidebar-brand {
    flex: none;
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 12px 12px 8px;
  }
  .logo {
    font-family: var(--mono);
    font-size: 13px;
    font-weight: 700;
    color: var(--accent);
    letter-spacing: 0.5px;
  }
  .brand-dot {
    width: 7px;
    height: 7px;
    border-radius: 50%;
    background: var(--ink-faint);
  }
  .brand-dot.live {
    background: var(--live);
    animation: led-pulse 2.6s ease-in-out infinite;
  }
  @media (prefers-reduced-motion: reduce) {
    .brand-dot.live {
      animation: none;
    }
  }
  .sidebar-filter {
    flex: none;
    padding: 0 10px 10px;
    border-bottom: 1px solid var(--edge);
  }
  .sidebar-filter input {
    width: 100%;
    background: var(--bg);
    border: 1px solid var(--edge);
    color: var(--ink);
    font-family: var(--mono);
    font-size: 11.5px;
    padding: 6px 8px;
    outline: none;
    transition: border-color 0.12s ease;
  }
  .sidebar-filter input::placeholder {
    color: var(--ink-faint);
  }
  .sidebar-filter input:focus {
    border-color: color-mix(in srgb, var(--accent) 55%, var(--edge));
  }
  .sidebar-scroll {
    flex: 1 1 auto;
    overflow-y: auto;
    padding: 4px 0 12px;
  }

  .nav-section {
    border-bottom: 1px solid var(--edge);
  }
  .nav-section:last-child {
    border-bottom: none;
  }
  .section-head {
    width: 100%;
    display: flex;
    align-items: center;
    gap: 7px;
    padding: 9px 10px;
    font: inherit;
    font-family: var(--mono);
    font-size: 11px;
    letter-spacing: 0.08em;
    text-transform: uppercase;
    color: var(--ink-dim);
    background: none;
    border: 0;
    cursor: pointer;
    text-align: left;
  }
  .section-name {
    flex: 1 1 auto;
  }
  .count {
    font-family: var(--mono);
    font-size: 10px;
    color: var(--ink-faint);
    background: var(--elev, var(--panel));
    border: 1px solid var(--edge);
    padding: 0 5px;
    font-variant-numeric: tabular-nums;
  }
  .section-empty {
    padding: 4px 12px 8px 26px;
    font-family: var(--mono);
    font-size: 11px;
    color: var(--ink-faint);
    font-style: italic;
  }

  /* A shared CSS chevron; rotates open. */
  .caret {
    width: 6px;
    height: 6px;
    flex: none;
    border-right: 1.4px solid var(--ink-faint);
    border-bottom: 1.4px solid var(--ink-faint);
    transform: rotate(-45deg);
    transition: transform 0.12s ease;
  }
  .caret.open {
    transform: rotate(45deg);
  }
  @media (prefers-reduced-motion: reduce) {
    .caret {
      transition: none;
    }
  }

  .session-head {
    width: 100%;
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 6px 10px 6px 20px;
    font: inherit;
    font-size: 12px;
    color: var(--ink);
    background: none;
    border: 0;
    cursor: pointer;
    text-align: left;
  }
  .session-head:hover {
    background: var(--elev, var(--panel));
  }
  .session-name {
    flex: 1 1 auto;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .session-age {
    flex: none;
    font-family: var(--mono);
    font-size: 10px;
    color: var(--ink-faint);
    font-variant-numeric: tabular-nums;
  }

  .run-row {
    width: 100%;
    display: flex;
    align-items: center;
    gap: 7px;
    padding: 5px 10px 5px 34px;
    font: inherit;
    background: none;
    border: 0;
    border-left: 2px solid transparent;
    cursor: pointer;
    text-align: left;
  }
  .run-row:hover {
    background: var(--elev, var(--panel));
  }
  .run-row.selected {
    background: var(--accent-soft);
    border-left-color: var(--accent);
  }
  .run-intent {
    flex: 1 1 auto;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--ink-dim);
    font-size: 12px;
  }
  .run-row.selected .run-intent {
    color: var(--ink);
  }
  .run-time {
    flex: none;
    font-family: var(--mono);
    font-size: 10px;
    color: var(--ink-faint);
    font-variant-numeric: tabular-nums;
  }

  /* resources + recordings rows */
  .res-row,
  .rec-row {
    width: 100%;
    display: flex;
    align-items: center;
    gap: 7px;
    padding: 6px 10px 6px 20px;
    font: inherit;
    background: none;
    border: 0;
    border-left: 2px solid transparent;
    cursor: pointer;
    text-align: left;
  }
  .res-row:hover,
  .rec-row:hover {
    background: var(--elev, var(--panel));
  }
  .res-row.selected,
  .rec-row.selected {
    background: var(--accent-soft);
    border-left-color: var(--accent);
  }
  .res-icon {
    flex: none;
    width: 14px;
    text-align: center;
    font-family: var(--mono);
    font-size: 11px;
    color: var(--ink-faint);
  }
  .res-name {
    flex: 1 1 auto;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--ink);
    font-size: 12px;
  }
  .res-meta {
    flex: none;
    font-family: var(--mono);
    font-size: 10px;
    color: var(--ink-faint);
    font-variant-numeric: tabular-nums;
  }
</style>
