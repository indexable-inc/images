<script lang="ts">
  import { stripAnsi } from '$lib/ansi';
  import { store, timeline, SCOPE_SEP } from '$lib/stream.svelte';
  import { ui, focusPane, humanAge } from '$lib/ui.svelte';
  import { rendererFor } from '$lib/renderers';
  import CodeBlock from './CodeBlock.svelte';
  import ExecBody from './ExecBody.svelte';
  import InlineTrace from './InlineTrace.svelte';
  import type { Pane } from '$lib/types';

  // The feed: a chronological timeline of panes, newest first. Each block puts the
  // input on the left and its execution/output on the right — for an exec that is
  // source | output. The output is the larger, fuller column; the code is the
  // supporting one. No per-block title (the code column *is* the input, so a title
  // taken from the source's first line would just be noise). Non-exec panes have
  // no input column, so their output spans the block. Click a block to open it
  // fullscreen.

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

  // Ages read against wall-clock while following live, else the scrubbed-to moment.
  const refMs = $derived(
    timeline.source === 'live' && timeline.following ? ui.clock : timeline.position || timeline.maxTs,
  );

  // Panes newest-first. created_at is the stamp; ties break by key for stability.
  const items = $derived(
    Object.keys(store.panes)
      .map((key) => {
        const sep = key.indexOf(SCOPE_SEP);
        const scope = sep === -1 ? '' : key.slice(0, sep);
        return { key, pane: { ...store.panes[key], key, scope } as Pane };
      })
      .sort(
        (a, b) =>
          (b.pane.created_at ?? 0) - (a.pane.created_at ?? 0) || (a.key < b.key ? -1 : 1),
      ),
  );
</script>

<div class="feed">
  {#if items.length === 0}
    <div class="feed-empty">{store.live ? 'no panes yet' : 'connecting…'}</div>
  {:else}
    <div class="feed-col">
      {#each items as it (it.key)}
        {@const p = it.pane}
        {@const k = p.kind ?? 'data'}
        {@const traceArr = k === 'exec' ? parseTrace(p.trace) : []}
        {@const traced = k === 'exec' && !!p.source && traceArr.length > 0}
        {@const hasCode = k === 'exec' && !!p.source && !traced}
        <article class="entry">
          <button class="entry-meta" onclick={() => focusPane(it.key)} title="open fullscreen">
            <span class="entry-led" class:live={ledLive(p)} class:run={ledRun(p)} class:err={ledErr(p)}></span>
            <span class="entry-tag">{tag(p)}</span>
            {#if p.subtitle}<span class="entry-sub">{p.subtitle}</span>{/if}
            <span class="entry-spacer"></span>
            <span class="entry-age">{humanAge(p.created_at, refMs)}</span>
          </button>

          {#if traced}
            <!-- Inline-trace: code full width; each printed line shows its output's
                 first line inline (gray), full output on hover (an overlay, so
                 nothing shifts). stderr (a traceback) has no line → stacks below. -->
            <div class="entry-box entry-trace-box">
              <InlineTrace source={p.source ?? ''} lang={p.lang ?? 'text'} trace={traceArr} />
              {#if p.stderr}<pre class="exec-out err trace-stderr">{stripAnsi(p.stderr)}</pre>{/if}
            </div>
          {:else if hasCode}
            <!-- No per-line trace (older producer / subprocess output): code on top,
                 its output stacked below — no fake side-by-side alignment. -->
            <div class="entry-box entry-stack">
              <div class="entry-code"><CodeBlock code={p.source ?? ''} lang={p.lang ?? 'text'} /></div>
              <ExecBody pane={p} chrome={false} expanded />
            </div>
          {:else if k === 'exec'}
            <div class="entry-box cell cell-out"><ExecBody pane={p} chrome={false} expanded /></div>
          {:else}
            {@const Body = rendererFor(k)}
            <div class="entry-box pane entry-body" class:term={k === 'terminal'} style={k === 'terminal' ? 'font-size: 13px;' : ''}>
              <div class="body" class:term-body={k === 'terminal'} class:html-body={k === 'html'}>
                <Body pane={p} />
              </div>
            </div>
          {/if}
        </article>
      {/each}
    </div>
  {/if}
</div>
