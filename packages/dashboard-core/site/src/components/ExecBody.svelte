<script lang="ts">
  import { stripAnsi } from '$lib/ansi';
  import type { Pane } from '$lib/types';
  import CodeBlock from './CodeBlock.svelte';

  // If a captured stream is a single JSON value, return it pretty-printed so it can
  // be syntax-highlighted; else null and it renders as plain text. Guarded on the
  // first non-space char so ordinary log output never pays for a failed parse.
  function asJson(text: string): string | null {
    const t = text.trim();
    if (t.length < 2 || (t[0] !== '{' && t[0] !== '[')) return null;
    try {
      return JSON.stringify(JSON.parse(t), null, 2);
    } catch {
      return null;
    }
  }

  // The exec renderer. The captured output is the point — the run's result and
  // stdout/stderr — so it leads; the source sits behind a quiet toggle, hidden by
  // default (you rarely need to re-read the code that produced the output). ANSI
  // escapes from a spawned process are stripped so the output reads as plain text
  // rather than raw control sequences. `expanded` lets the detail/focus view grow
  // the body past the card's cap; it does not reveal the source.
  // `chrome` draws the source toggle + status footer; the feed sets it false and
  // shows the source in its own column, so the renderer is just the output there.
  let { pane, expanded = false, chrome = true }: { pane: Pane; expanded?: boolean; chrome?: boolean } =
    $props();

  // Source stays collapsed until asked for, on cards and in the detail alike.
  let showSource = $state(false);

  const stdout = $derived(stripAnsi(pane.stdout ?? ''));
  const stderr = $derived(stripAnsi(pane.stderr ?? ''));
  const result = $derived(pane.result ?? '');
  // Auto-detected JSON (pretty-printed) for the result/stdout streams, else null.
  const resultJson = $derived(asJson(result));
  const stdoutJson = $derived(asJson(stdout));
  const source = $derived(pane.source ?? '');
  const lang = $derived(pane.lang ?? '');
  const running = $derived(pane.running === true);
  const empty = $derived(!stdout && !stderr && !result);

  // The card is a drag handle; interactive controls must not start a drag.
  function swallow(e: PointerEvent): void {
    e.stopPropagation();
  }
</script>

<div class="exec" class:expanded>
  <!-- Output first and unadorned: the result and captured streams are the point.
       Result leads (it is the answer for an eval), then stdout, then stderr. -->
  {#if empty}
    <div class="exec-empty">{running ? '· running…' : '· no output'}</div>
  {:else}
    {#if result}
      {#if resultJson}<div class="exec-out res exec-json"><CodeBlock code={resultJson} lang="json" /></div>
      {:else}<pre class="exec-out res">{result}</pre>{/if}
    {/if}
    {#if stdout}
      {#if stdoutJson}<div class="exec-out exec-json"><CodeBlock code={stdoutJson} lang="json" /></div>
      {:else}<pre class="exec-out">{stdout}</pre>{/if}
    {/if}
    {#if stderr}<pre class="exec-out err">{stderr}</pre>{/if}
  {/if}

  <!-- The source is demoted to a quiet toggle below the output (you rarely need
       to re-read the code that produced it). Status lives on the entry's LED.
       Skipped when `chrome` is off (the feed shows the source in its own column). -->
  {#if chrome && (source || lang || running || pane.ok === false)}
    <div class="exec-foot">
      {#if source}
        <button class="exec-srctoggle" class:on={showSource} onpointerdown={swallow} onclick={() => (showSource = !showSource)}>
          {showSource ? 'hide source' : 'source'}
        </button>
      {/if}
      {#if lang}<span class="exec-lang">{lang}</span>{/if}
      <span class="exec-spacer"></span>
      {#if running}<span class="exec-state run">running…</span>
      {:else if pane.ok === false}<span class="exec-state err">error</span>{/if}
    </div>
  {/if}

  {#if chrome && showSource}
    <pre class="exec-source">{source}</pre>
  {/if}
</div>
