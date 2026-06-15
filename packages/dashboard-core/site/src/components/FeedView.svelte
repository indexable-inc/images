<script lang="ts">
  import { onMount } from 'svelte';
  import { stripAnsi } from '$lib/ansi';
  import { store, timeline, SCOPE_SEP } from '$lib/stream.svelte';
  import { ui, focusPane, humanAge, humanDuration } from '$lib/ui.svelte';
  import { setListNav } from '$lib/keys.svelte';
  import { rendererFor } from '$lib/renderers';
  import CodeBlock from './CodeBlock.svelte';
  import ExecBody from './ExecBody.svelte';
  import InlineTrace from './InlineTrace.svelte';
  import type { Pane } from '$lib/types';

  // The feed: a chronological timeline of panes, oldest first (newest at the
  // bottom, so it reads as a live log growing downward). Each block puts the
  // input on the left and its execution/output on the right — for an exec that is
  // source | output. The output is the larger, fuller column; the code is the
  // supporting one. Each block leads with the pane title, which for an exec is the
  // caller's `intent` (a plain-language statement of what the run is for), so the
  // feed reads as a list of intents rather than code; the lang/op/session sit
  // beside it as quiet meta. Non-exec panes have no input column, so their output
  // spans the block. Click a block to open it fullscreen.

  function ledLive(p: Pane): boolean {
    const kind = p.kind ?? 'data';
    if (kind === 'exec') return p.ok === true;
    if (kind === 'terminal') return p.alive !== false;
    return true;
  }
  function ledRun(p: Pane): boolean {
    return (p.kind ?? 'data') === 'exec' && p.running === true;
  }
  function ledErr(p: Pane): boolean {
    const kind = p.kind ?? 'data';
    return (kind === 'exec' && p.ok === false) || (kind === 'terminal' && p.alive === false);
  }
  // The thin meta tag: an exec's language, else its kind.
  function tag(p: Pane): string {
    const kind = p.kind ?? 'data';
    return kind === 'exec' ? p.lang || 'exec' : kind;
  }

  // The hub stores an exec's inline-trace as JSON text; parse it back to the
  // `{line, text}[]` the trace view consumes. Malformed/absent → no trace.
  function parseTrace(raw: string | undefined): { line: number; text: string }[] {
    if (!raw) return [];
    try {
      const value = JSON.parse(raw);
      return Array.isArray(value) ? value : [];
    } catch {
      return [];
    }
  }

  // Pull the essentials out of a Python traceback so a failed run shows *where* it
  // broke, not a wall of text: the final `Type: message` line, plus each frame in
  // the user's own exec source (filename `<ix-mcp exec>`) mapped to that source
  // line. A transport error (no traceback) yields just its message, no frames.
  type ErrInfo = { message: string; frames: { line: number; text: string }[] };
  function parseError(stderr: string | undefined, source: string | undefined): ErrInfo | null {
    const text = stripAnsi(stderr ?? '').trimEnd();
    if (!text) return null;
    const lines = text.split('\n');
    const message = lines.filter((l) => l.trim()).at(-1)?.trim() ?? text;
    const src = (source ?? '').split('\n');
    const frames: { line: number; text: string }[] = [];
    for (const m of text.matchAll(/File "([^"]*)", line (\d+)/g)) {
      const file = m[1];
      const line = Number(m[2]);
      // Only frames inside the run's own source have a line we can point at; the
      // exec wrapper names them `<ix-mcp exec>` (or another synthetic `<...>`).
      if (file.includes('ix-mcp') || file.startsWith('<')) {
        frames.push({ line, text: (src[line - 1] ?? '').trim() });
      }
    }
    return { message, frames };
  }

  // Ages read against wall-clock while following live, else the scrubbed-to moment.
  const refMs = $derived(
    timeline.source === 'live' && timeline.following ? ui.clock : timeline.position || timeline.maxTs,
  );

  // Master-detail: the left column is a quiet list of intents (one line per run);
  // the selected entry's output/error/source renders in the detail panel on the
  // right, which follows the selection. `showCode` (per pane key) reveals the
  // source inside that panel.
  let showCode: Record<string, boolean> = $state({});
  function toggleCode(e: MouseEvent, key: string): void {
    e.stopPropagation();
    showCode[key] = !showCode[key];
  }

  // The short run id from a pane key (`scope<0x1f>id`): the trailing id, shown as
  // quiet meta so a human can still correlate a row to a `jobs['<id>']` without it
  // being the headline (the title/intent is).
  function shortId(key: string): string {
    const sep = key.indexOf(SCOPE_SEP);
    const id = sep === -1 ? key : key.slice(sep + 1);
    return id.includes('/') ? id.slice(id.lastIndexOf('/') + 1) : id;
  }

  // A namespace pane (the kernel's live globals) is not a run — it has its own
  // rail view, so it must never appear interleaved in the chronological feed.
  function isNamespace(p: Pane): boolean {
    return (p.kind ?? 'data') === 'data' && p.renderer === 'namespace';
  }

  // A run's rich output (a table/plot/image) is published as a separate
  // `<id>/out` html pane beside its exec pane. It is an *attachment* to the run,
  // not its own entry, so it must not appear as a duplicate feed row — it is folded
  // into the run's detail panel instead (see `outPane` below).
  function isOutputAttachment(key: string): boolean {
    const sep = key.indexOf(SCOPE_SEP);
    const id = sep === -1 ? key : key.slice(sep + 1);
    return id.endsWith('/out');
  }

  // Panes oldest-first, so the newest run lands at the bottom and the list reads
  // as a live log that grows downward. created_at is the stamp; ties break by key
  // for stability. Namespace panes and per-run output attachments are excluded —
  // the former has its own view, the latter folds into its run's detail.
  const items = $derived(
    Object.keys(store.panes)
      .filter((key) => !isOutputAttachment(key))
      .map((key) => {
        const sep = key.indexOf(SCOPE_SEP);
        const scope = sep === -1 ? '' : key.slice(0, sep);
        return { key, pane: { ...store.panes[key], key, scope } as Pane };
      })
      .filter((it) => !isNamespace(it.pane))
      .sort(
        (a, b) =>
          (a.pane.created_at ?? 0) - (b.pane.created_at ?? 0) || (a.key < b.key ? -1 : 1),
      ),
  );

  // How many runs are currently executing, for the header's active badge.
  const activeCount = $derived(items.filter((it) => ledRun(it.pane)).length);

  // `/` filter: a case-insensitive substring match over the title, id and tag, so
  // a long feed narrows to the run you mean. Empty query shows everything.
  let query = $state('');
  let filterEl: HTMLInputElement | undefined;
  const filtered = $derived.by(() => {
    const q = query.trim().toLowerCase();
    if (!q) return items;
    return items.filter((it) => {
      const p = it.pane;
      return (
        (p.title ?? '').toLowerCase().includes(q) ||
        shortId(it.key).toLowerCase().includes(q) ||
        tag(p).toLowerCase().includes(q)
      );
    });
  });

  // Vim navigation lives in the central keymap (lib/keys.svelte); this view just
  // registers what j/k/gg/G/o do here. `o` (or Enter / l) toggles the source in the
  // detail. Selection follows clicks too, so mouse and keyboard agree.
  let selectedKey: string | null = $state(null);
  const selected = $derived(filtered.find((it) => it.key === selectedKey) ?? null);
  // The selected run's rich-output attachment, if any: the `<key>/out` html pane
  // published beside the exec (a table/plot/image). Rendered inside the detail so
  // the run is one entry, not two.
  const selectedOut = $derived.by<Pane | null>(() => {
    if (!selected) return null;
    const rec = store.panes[selected.key + '/out'];
    return rec ? ({ ...rec, key: selected.key + '/out', scope: selected.pane.scope } as Pane) : null;
  });
  function scrollSelected(): void {
    const key = selectedKey;
    if (key == null) return;
    queueMicrotask(() =>
      document
        .querySelector(`li.entry[data-key="${CSS.escape(key)}"]`)
        ?.scrollIntoView({ block: 'nearest' }),
    );
  }
  $effect(() => {
    // Keep the selection valid as panes come and go (or the filter narrows the
    // list); default to the newest row (the last, since the list grows downward)
    // and scroll it into view.
    if (filtered.length === 0) {
      selectedKey = null;
    } else if (!filtered.some((it) => it.key === selectedKey)) {
      selectedKey = filtered[filtered.length - 1].key;
      scrollSelected();
    }
  });
  function selectIndex(i: number): void {
    if (!filtered.length) return;
    const n = Math.max(0, Math.min(filtered.length - 1, i));
    selectedKey = filtered[n].key;
    scrollSelected();
  }
  function move(delta: number): void {
    const i = filtered.findIndex((it) => it.key === selectedKey);
    selectIndex((i < 0 ? 0 : i) + delta);
  }
  function openSelected(): void {
    if (selectedKey) showCode[selectedKey] = !showCode[selectedKey];
  }

  // Register this view's motions with the global keymap while it is mounted.
  onMount(() => {
    setListNav({
      move,
      top: () => selectIndex(0),
      bottom: () => selectIndex(filtered.length - 1),
      open: openSelected,
      filter: () => filterEl?.focus(),
    });
    return () => setListNav(null);
  });
</script>

<div class="feedview">
  <header class="view-head">
    <h1 class="view-title">Jobs</h1>
    {#if items.length}<span class="view-meta">{items.length} {items.length === 1 ? 'run' : 'runs'}</span>{/if}
    {#if activeCount}<span class="view-active"><i></i>{activeCount} active</span>{/if}
    <span class="view-spacer"></span>
    <!-- The `/` filter. Esc (handled by the keymap) blurs it back to navigation. -->
    <input
      class="view-filter"
      type="text"
      placeholder="/ filter"
      bind:value={query}
      bind:this={filterEl}
      spellcheck="false"
      autocomplete="off"
      aria-label="filter runs"
    />
  </header>
  <div class="feed feed-split">
  {#if items.length === 0}
    <div class="feed-empty">{store.live ? 'no panes yet' : 'connecting…'}</div>
  {:else if filtered.length === 0}
    <div class="feed-empty">no runs match “{query}”</div>
  {:else}
    <!-- Left: the timeline list. One quiet line per run; the rail threads the
         dots. Selecting a row drives the detail panel; it never expands inline. -->
    <ol class="feed-list">
      {#each filtered as it (it.key)}
        {@const p = it.pane}
        {@const running = ledRun(p)}
        {@const isErr = ledErr(p)}
        <li class="entry" class:err={isErr} class:selected={selectedKey === it.key} data-key={it.key}>
          <button class="entry-row" onclick={() => (selectedKey = it.key)} title={`${tag(p)}${p.subtitle ? ' · ' + p.subtitle : ''}`}>
            <span class="entry-dot" class:live={ledLive(p)} class:run={running} class:err={isErr}></span>
            <span class="entry-main">
              <span class="entry-title" title={p.title}>{p.title || '(pane)'}</span>
              <span class="entry-sub">{shortId(it.key)}<span class="entry-sub-tag"> · {tag(p)}</span></span>
            </span>
            {#if running}
              <span class="entry-now">running</span>
            {:else if p.duration_ms != null}
              <span class="entry-age" title="execution time">{humanDuration(p.duration_ms)}</span>
            {:else}
              <span class="entry-age">{humanAge(p.created_at, refMs)}</span>
            {/if}
          </button>
        </li>
      {/each}
    </ol>

    <!-- Right: detail for the selected entry. Output leads; for an error the
         parsed failure (message + source line) sits on top; `code` reveals the
         source (inline-trace when attributed, else a highlighted block). -->
    <div class="feed-detail">
      {#if selected}
        {@const p = selected.pane}
        {@const k = p.kind ?? 'data'}
        {@const traceArr = k === 'exec' ? parseTrace(p.trace) : []}
        {@const traced = k === 'exec' && !!p.source && traceArr.length > 0}
        {@const hasSource = k === 'exec' && !!p.source}
        {@const isErr = ledErr(p)}
        {@const codeOpen = showCode[selected.key] === true}
        {@const errInfo = isErr ? parseError(p.stderr, p.source) : null}
        <div class="detail-head">
          <span class="entry-dot" class:live={ledLive(p)} class:run={ledRun(p)} class:err={isErr}></span>
          <span class="detail-title" class:err={isErr}>{p.title || '(pane)'}</span>
        </div>
        <div class="detail-meta">
          {#if ledRun(p)}running{:else if p.duration_ms != null}{humanDuration(p.duration_ms)}{:else}{humanAge(p.created_at, refMs)}{/if}
          {#if tag(p)} · {tag(p)}{/if}{#if p.subtitle} · {p.subtitle}{/if}
        </div>

        <div class="detail-body">
          {#if k === 'exec'}
            {#if errInfo}
              <div class="entry-fail">
                <div class="fail-msg">{errInfo.message}</div>
                {#each errInfo.frames as fr}
                  <div class="fail-frame">
                    <span class="fail-line">{fr.line}</span>
                    <code class="fail-src">{fr.text}</code>
                  </div>
                {/each}
              </div>
            {/if}
            <div class="entry-box cell cell-out"><ExecBody pane={p} chrome={false} expanded /></div>
            {#if selectedOut}
              {@const OutBody = rendererFor(selectedOut.kind, selectedOut.renderer)}
              <div class="entry-box pane entry-body detail-out">
                <div class="body html-body"><OutBody pane={selectedOut} /></div>
              </div>
            {/if}
            {#if hasSource}
              <button
                class="entry-code-toggle"
                class:on={codeOpen}
                onclick={(e) => toggleCode(e, selected.key)}
                title={codeOpen ? 'hide code' : 'show code'}
              >{codeOpen ? '▾' : '▸'} code</button>
              {#if codeOpen}
                {#if traced}
                  <div class="entry-box entry-trace-box entry-code-reveal">
                    <InlineTrace source={p.source ?? ''} lang={p.lang ?? 'text'} trace={traceArr} />
                    {#if p.stderr}<pre class="exec-out err trace-stderr">{stripAnsi(p.stderr)}</pre>{/if}
                  </div>
                {:else}
                  <div class="entry-box entry-code entry-code-reveal"><CodeBlock code={p.source ?? ''} lang={p.lang ?? 'text'} /></div>
                {/if}
              {/if}
            {/if}
          {:else}
            {@const Body = rendererFor(k, p.renderer)}
            <div class="entry-box pane entry-body" class:term={k === 'terminal'} style={k === 'terminal' ? 'font-size: 13px;' : ''}>
              <div class="body" class:term-body={k === 'terminal'} class:html-body={k === 'html'}>
                <Body pane={p} />
              </div>
            </div>
          {/if}
        </div>
      {:else}
        <div class="detail-empty">select an entry</div>
      {/if}
    </div>
  {/if}
  </div>
</div>
