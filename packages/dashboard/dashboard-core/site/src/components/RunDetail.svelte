<script lang="ts">
  // The center stage for a selected run: a header (LED, intent, status pill,
  // duration, start time, session breadcrumb) over stacked foldable panels —
  // code, stdout, result (only when it diverges from stdout AND there is no
  // rich attachment), and output (the `<key>/out` attachment). Panels default
  // collapsed with a one-line preview in each summary, so a run scans without
  // opening anything. The exec-detail behaviour is the feed's, refactored here:
  // inline trace when attributed, else a code block, and a parsed failure on
  // top for an error.
  import { stripAnsi } from '$lib/ansi';
  import { store, timeline } from '$lib/stream.svelte';
  import { ui, humanDuration, humanTime } from '$lib/ui.svelte';
  import { rendererFor } from '$lib/renderers';
  import { ledOf, kindOf, withKey } from '$lib/run';
  import { parseTrace, parseError } from '$lib/exec';
  import { shortScope } from '$lib/feed-sessions';
  import CodeBlock from './CodeBlock.svelte';
  import ExecBody from './ExecBody.svelte';
  import InlineTrace from './InlineTrace.svelte';
  import type { Pane } from '$lib/types';

  let { pane, sessionLabel }: { pane: Pane; sessionLabel: string } = $props();

  const k = $derived(kindOf(pane));
  const led = $derived(ledOf(pane));
  const running = $derived(led === 'running');
  const isErr = $derived(led === 'error');

  // Ages read against wall-clock while following live, else the scrubbed-to moment.
  const refMs = $derived(
    timeline.source === 'live' && timeline.following ? ui.clock : timeline.position || timeline.maxTs,
  );

  const statusText = $derived(running ? 'running' : isErr ? 'error' : 'ok');
  const startStamp = $derived(humanTime(pane.created_at, refMs));

  // The run's rich-output attachment (`<key>/out`), folded in rather than shown as
  // a duplicate entry.
  const outPane = $derived.by<Pane | null>(() => {
    const rec = store.panes[pane.key + '/out'];
    return rec ? withKey(pane.key + '/out', rec, pane.scope) : null;
  });

  // Exec-detail derivations (mirrors the feed's, one place now).
  const traceArr = $derived(k === 'exec' ? parseTrace(pane.trace) : []);
  const traced = $derived(k === 'exec' && !!pane.source && traceArr.length > 0);
  const hasSource = $derived(k === 'exec' && !!pane.source);
  const errInfo = $derived(isErr ? parseError(pane.stderr, pane.source) : null);
  const stdoutTxt = $derived(k === 'exec' ? stripAnsi(pane.stdout ?? '').trim() : '');
  const resultTxt = $derived(k === 'exec' ? (pane.result ?? '').trim() : '');
  const hasStreamOut = $derived(!!stdoutTxt || !!stripAnsi(pane.stderr ?? '').trim());
  const resultIsPrimary = $derived(!hasStreamOut && !outPane && !!resultTxt);
  const resultShownInline = $derived(!traced && resultIsPrimary);
  // A file-view attachment IS the result rendered (the read's body), so showing
  // the result panel beside it would duplicate the output. Other attachments (a
  // displayed plot, a table) can coexist with a genuinely distinct result —
  // `display(df); "done"` — so only the file-view case suppresses it.
  const outIsResultView = $derived(outPane?.kind === 'data' && outPane?.renderer === 'file-view');
  const resultIsExtra = $derived(
    !!resultTxt && resultTxt !== stdoutTxt && !resultShownInline && !outIsResultView,
  );

  // One-line summary previews so a collapsed panel still scans.
  function firstLine(text: string): string {
    const line = text.slice(0, 200).split('\n', 1)[0] ?? '';
    return line.length > 80 ? line.slice(0, 80) + '…' : line;
  }
  const outputHint = $derived(firstLine(stdoutTxt || resultTxt) || 'stdout');
  // The attachment's summary: a file-view names its file and span; anything else
  // names its renderer/kind.
  const outHint = $derived.by(() => {
    if (!outPane) return '';
    if (outPane.kind === 'data' && outPane.renderer === 'file-view') {
      try {
        const v: unknown = JSON.parse(outPane.body ?? '');
        if (v && typeof v === 'object') {
          const fv = v as { label?: string; start?: number; end?: number };
          const span = fv.start != null && fv.end != null ? ` · ${fv.start}–${fv.end}` : '';
          return `${fv.label ?? 'file'}${span}`;
        }
      } catch {
        // fall through to the generic hint
      }
    }
    return outPane.renderer ?? outPane.kind ?? '';
  });
</script>

<div class="run-detail">
  <header class="run-header">
    <div class="run-header-top">
      <span class="led led-{led}"></span>
      <span class="run-title" class:err={isErr}>{pane.title || '(pane)'}</span>
      <div class="run-header-meta">
        <span class="pill" class:err={isErr} class:run={running}>{statusText}</span>
        {#if !running && pane.duration_ms != null}<span>{humanDuration(pane.duration_ms)}</span>{/if}
        {#if startStamp}<span>{startStamp}</span>{/if}
      </div>
    </div>
    <div class="run-subline">
      <span class="session-link">{sessionLabel || shortScope(pane.scope)}</span>
      {#if pane.subtitle} · {pane.subtitle}{/if}
    </div>
  </header>

  <div class="panels">
    {#if k === 'exec'}
      {#if errInfo}
        <div class="run-fail">
          <div class="fail-msg">{errInfo.message}</div>
          {#each errInfo.frames as fr (fr.line)}
            <div class="fail-frame">
              <span class="fail-line">{fr.line}</span>
              <code class="fail-src">{fr.text}</code>
            </div>
          {/each}
        </div>
      {/if}

      {#if traced}
        <!-- Inline trace: source with each line's output beside it, one combined
             view, so it stands in for both the code and output panels. -->
        <details class="panel">
          <summary><span class="caret"></span><span class="panel-label">code · output</span></summary>
          <div class="panel-body panel-body-flush">
            <InlineTrace source={pane.source ?? ''} lang={pane.lang ?? 'text'} trace={traceArr} />
            {#if pane.stderr}<pre class="exec-out err trace-stderr">{stripAnsi(pane.stderr)}</pre>{/if}
          </div>
        </details>
      {:else}
        {#if hasSource}
          <!-- Code collapsed by default: you usually read output, not the source. -->
          <details class="panel">
            <summary>
              <span class="caret"></span><span class="panel-label">code</span>
              <span class="panel-hint">{pane.lang || 'source'}</span>
            </summary>
            <div class="panel-body"><CodeBlock code={pane.source ?? ''} lang={pane.lang ?? 'text'} /></div>
          </details>
        {/if}
        {#if hasStreamOut || resultIsPrimary || running}
          <details class="panel">
            <!-- Labelled `stdout` when a rich attachment exists, so the run never
                 shows two panels both called `output`. -->
            <summary><span class="caret"></span><span class="panel-label">{outPane ? 'stdout' : 'output'}</span><span class="panel-hint">{outputHint}</span></summary>
            <div class="panel-body panel-body-flush">
              <ExecBody {pane} chrome={false} expanded hideResult={!resultIsPrimary} />
            </div>
          </details>
        {/if}
      {/if}

      {#if resultIsExtra}
        <!-- The result the model received, shown only when it adds something
             beyond stdout so the common case never duplicates the output above. -->
        <details class="panel">
          <summary><span class="caret"></span><span class="panel-label">result</span><span class="panel-hint">model view</span></summary>
          <div class="panel-body panel-body-flush"><pre class="exec-out res">{pane.result}</pre></div>
        </details>
      {/if}

      {#if outPane}
        {@const OutBody = rendererFor(outPane.kind, outPane.renderer)}
        <details class="panel">
          <summary><span class="caret"></span><span class="panel-label">output</span><span class="panel-hint">{outHint}</span></summary>
          <div class="panel-body panel-body-flush pane">
            <div class="body" class:html-body={outPane.kind === 'html'}><OutBody pane={outPane} /></div>
          </div>
        </details>
      {/if}
    {:else}
      <!-- A non-exec run pane (a data/html result): render its body directly. -->
      {@const Body = rendererFor(k, pane.renderer)}
      <div class="panel">
        <div class="panel-body panel-body-flush pane" class:term={k === 'terminal'}>
          <div class="body" class:term-body={k === 'terminal'} class:html-body={k === 'html'}>
            <Body {pane} />
          </div>
        </div>
      </div>
    {/if}
  </div>
</div>

<style>
  .run-detail {
    flex: 1 1 auto;
    min-width: 0;
    min-height: 0;
    overflow-y: auto;
    background: var(--bg);
    display: flex;
    flex-direction: column;
  }
  .run-header {
    flex: none;
    padding: 16px clamp(16px, 2.4vw, 24px) 14px;
    border-bottom: 1px solid var(--edge);
  }
  .run-header-top {
    display: flex;
    align-items: center;
    gap: 10px;
  }
  .run-header-top .led {
    width: 9px;
    height: 9px;
  }
  .run-title {
    font-size: 15px;
    font-weight: 500;
    color: var(--ink);
    min-width: 0;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .run-title.err {
    color: light-dark(#c4314b, #f0a0ad);
  }
  .run-header-meta {
    margin-left: auto;
    display: flex;
    align-items: center;
    gap: 12px;
    font-family: var(--mono);
    font-size: 11px;
    color: var(--ink-dim);
    flex: none;
  }
  .pill {
    border: 1px solid var(--edge);
    background: var(--elev, var(--panel));
    padding: 2px 7px;
    color: var(--ink-dim);
    letter-spacing: 0.02em;
  }
  .pill.run {
    color: var(--k-amber);
    border-color: color-mix(in srgb, var(--k-amber) 40%, var(--edge));
  }
  .pill.err {
    color: var(--dead);
    border-color: color-mix(in srgb, var(--dead) 40%, var(--edge));
  }
  .run-subline {
    margin-top: 6px;
    font-family: var(--mono);
    font-size: 11.5px;
    color: var(--ink-faint);
  }
  .run-subline .session-link {
    color: var(--ink-dim);
  }

  .panels {
    padding: 10px clamp(16px, 2.4vw, 24px) 44px;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }

  /* A foldable panel: a quiet caret + label row over its body. Flat — no strip
     background, no box; the body carries a single hairline frame. Uses native
     <details> so folding is free and CSS-only. */
  .panel > summary {
    list-style: none;
    cursor: pointer;
    display: flex;
    align-items: center;
    gap: 7px;
    padding: 6px 2px;
    user-select: none;
    min-width: 0;
  }
  .panel > summary::-webkit-details-marker {
    display: none;
  }
  /* A CSS chevron so no glyph font is needed; rotates open. */
  .panel > summary .caret {
    width: 6px;
    height: 6px;
    flex: none;
    border-right: 1.2px solid var(--ink-faint);
    border-bottom: 1.2px solid var(--ink-faint);
    transform: rotate(-45deg);
    transition: transform 0.12s ease;
  }
  .panel[open] > summary .caret {
    transform: rotate(45deg);
  }
  .panel-label {
    font-family: var(--mono);
    font-size: 11.5px;
    color: var(--ink-dim);
  }
  .panel > summary:hover .panel-label {
    color: var(--ink);
  }
  .panel-hint {
    margin-left: auto;
    font-family: var(--mono);
    font-size: 10.5px;
    color: var(--ink-faint);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    min-width: 0;
  }
  /* The hint is the collapsed row's preview; the open body says it better. */
  .panel[open] > summary .panel-hint {
    display: none;
  }
  .panel-body {
    padding: 10px 2px;
  }
  /* Renderer bodies (exec output, code, html frame) bring their own padding and
     background; frame them with one hairline. */
  .panel-body-flush {
    padding: 0;
    overflow: auto;
    max-height: 60vh;
    border: 1px solid var(--edge);
    border-radius: 4px;
    margin: 2px 0 8px;
  }
  .panel-body-flush.pane .body.html-body {
    height: 300px;
  }

  /* Parsed failure: the message then the source line(s) it came from. */
  .run-fail {
    display: flex;
    flex-direction: column;
    gap: 3px;
    padding: 2px 2px 4px;
  }
  .fail-msg {
    font-family: var(--mono);
    font-size: 12px;
    color: light-dark(#c4314b, #f0a0ad);
    white-space: pre-wrap;
    word-break: break-word;
  }
  .fail-frame {
    display: flex;
    align-items: baseline;
    gap: 10px;
    font-family: var(--mono);
    font-size: 12px;
  }
  .fail-line {
    flex: none;
    min-width: 1.6em;
    text-align: right;
    color: var(--ink-faint);
    font-variant-numeric: tabular-nums;
  }
  .fail-line::before {
    content: 'line ';
    color: var(--ink-faint);
  }
  .fail-src {
    color: var(--ink-dim);
    white-space: pre;
    overflow: hidden;
    text-overflow: ellipsis;
  }
</style>
