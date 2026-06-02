<script lang="ts">
  import { untrack } from 'svelte';
  import type { Pane } from '$lib/types';

  // The exec renderer: a box showing the run's captured stdout and stderr, with
  // the source behind it (revealed by the toggle, or always shown in the focus
  // view via `expanded`). ANSI escapes from a spawned process are stripped so
  // the output reads as plain text rather than raw control sequences.
  let { pane, expanded = false }: { pane: Pane; expanded?: boolean } = $props();

  // `expanded` seeds the toggle's initial state and is constant per mount (true
  // in the focus view, false on a card); untrack makes that one-shot read explicit.
  let showSource = $state(untrack(() => expanded));

  // Strip CSI/SGR and other escape sequences; exec output is a raw stream, not
  // the SGR-encoded grid the terminal renderer expects.
  function strip(text: string): string {
    // eslint-disable-next-line no-control-regex
    return text.replace(/\x1b\[[0-9;?]*[ -/]*[@-~]/g, '').replace(/\x1b[@-Z\\-_]/g, '');
  }

  const stdout = $derived(strip(pane.stdout ?? ''));
  const stderr = $derived(strip(pane.stderr ?? ''));
  const result = $derived(pane.result ?? '');
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
  <div class="exec-bar">
    <button
      class="exec-toggle"
      class:on={showSource}
      onpointerdown={swallow}
      onclick={() => (showSource = !showSource)}
    >
      {showSource ? 'hide source' : 'show source'}
    </button>
    {#if lang}<span class="exec-lang">{lang}</span>{/if}
    <span class="exec-spacer"></span>
    {#if running}<span class="exec-state run">running…</span>
    {:else if pane.ok === false}<span class="exec-state err">error</span>
    {:else}<span class="exec-state ok">done</span>{/if}
  </div>

  {#if showSource}
    <pre class="exec-source">{source}</pre>
  {/if}

  {#if empty}
    <div class="exec-empty">{running ? '· running…' : '· no output'}</div>
  {:else}
    {#if stdout}<pre class="exec-out">{stdout}</pre>{/if}
    {#if stderr}<pre class="exec-out err">{stderr}</pre>{/if}
    {#if result}<pre class="exec-out res">{result}</pre>{/if}
  {/if}
</div>
