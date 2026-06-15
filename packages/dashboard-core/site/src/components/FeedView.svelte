<script lang="ts">
  import { stripAnsi } from '$lib/ansi';
  import { store, timeline, SCOPE_SEP } from '$lib/stream.svelte';
  import { ui, focusPane, humanAge, humanDuration } from '$lib/ui.svelte';
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

  // Panes oldest-first, so the newest run lands at the bottom and the list reads
  // as a live log that grows downward. created_at is the stamp; ties break by key
  // for stability. Namespace panes are excluded — they belong to the Namespace view.
  const items = $derived(
    Object.keys(store.panes)
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

  // Vim navigation: `j`/`k` move the selection down/up; the detail panel follows.
  // `o` (or Enter) toggles the source inside the detail. Selection follows clicks
  // too, so mouse and keyboard agree.
  let selectedKey: string | null = $state(null);
  const selected = $derived(items.find((it) => it.key === selectedKey) ?? null);
  $effect(() => {
    // Keep the selection valid as panes come and go; default to the newest row
    // (now the last, since the list grows downward) and scroll it into view.
    if (items.length === 0) {
      selectedKey = null;
    } else if (!items.some((it) => it.key === selectedKey)) {
      selectedKey = items[items.length - 1].key;
      const key = selectedKey;
      queueMicrotask(() =>
        document
          .querySelector(`li.entry[data-key="${CSS.escape(key)}"]`)
          ?.scrollIntoView({ block: 'nearest' }),
      );
    }
  });
  function move(delta: number): void {
    if (!items.length) return;
    const i = items.findIndex((it) => it.key === selectedKey);
    const next = Math.max(0, Math.min(items.length - 1, (i < 0 ? 0 : i) + delta));
    selectedKey = items[next].key;
    // Defer until the class lands so the freshly-selected row is the scroll target.
    queueMicrotask(() => {
      if (selectedKey == null) return;
      document
        .querySelector(`li.entry[data-key="${CSS.escape(selectedKey)}"]`)
        ?.scrollIntoView({ block: 'nearest' });
    });
  }
  function onKeydown(e: KeyboardEvent): void {
    const t = e.target as HTMLElement | null;
    if (e.metaKey || e.ctrlKey || e.altKey) return;
    if (t && (t.tagName === 'INPUT' || t.tagName === 'TEXTAREA' || t.isContentEditable)) return;
    if (e.key === 'j') {
      e.preventDefault();
      move(1);
    } else if (e.key === 'k') {
      e.preventDefault();
      move(-1);
    } else if (e.key === 'o' || e.key === 'Enter') {
      if (!selectedKey) return;
      e.preventDefault();
      showCode[selectedKey] = !showCode[selectedKey];
    }
  }
</script>

<svelte:window onkeydown={onKeydown} />

<div class="feedview">
  <header class="view-head">
    <h1 class="view-title">Jobs</h1>
    {#if items.length}<span class="view-meta">{items.length} {items.length === 1 ? 'run' : 'runs'}</span>{/if}
    {#if activeCount}<span class="view-active"><i></i>{activeCount} active</span>{/if}
  </header>
  <div class="feed feed-split">
  {#if items.length === 0}
    <div class="feed-empty">{store.live ? 'no panes yet' : 'connecting…'}</div>
  {:else}
    <!-- Left: the timeline list. One quiet line per run; the rail threads the
         dots. Selecting a row drives the detail panel; it never expands inline. -->
    <ol class="feed-list">
      {#each items as it (it.key)}
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
