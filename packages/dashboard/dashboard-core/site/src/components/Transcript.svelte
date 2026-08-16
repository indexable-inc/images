<script lang="ts">
  // The chat-shaped transcript: the agent's own session log, normalized by the
  // tui producer into append-only rows (role, text, tool, usage). Registered as
  // the `transcript` data renderer and mounted inside AgentCard.
  //
  // Scroll: the reader's position is theirs. Rows are keyed by index (within
  // the producer's window rows only append, so an index is stable) and the view
  // auto-scrolls only when it was already at the bottom before the new row --
  // a reader who scrolled up to re-read something stays put while the
  // transcript grows underneath.
  import { tick } from 'svelte';
  import { parseTranscript } from '$lib/agent';
  import type { Pane } from '$lib/types';

  let { pane }: { pane: Pane } = $props();
  let scroller: HTMLElement | undefined = $state();
  // Deliberately not $state: reading it back in the effect below must not make
  // the effect depend on it, and nothing renders it.
  let atBottom = true;

  const transcript = $derived(parseTranscript(pane.body));

  function onScroll(): void {
    const el = scroller;
    if (!el) return;
    atBottom = el.scrollTop + el.clientHeight >= el.scrollHeight - 8;
  }

  $effect(() => {
    void transcript.entries.length;
    const el = scroller;
    if (!el || !atBottom) return;
    // After the DOM has the new rows, else scrollHeight is the old height.
    void tick().then(() => {
      el.scrollTop = el.scrollHeight;
    });
  });
</script>

<div class="transcript" bind:this={scroller} onscroll={onScroll}>
  {#if !transcript.entries.length}
    <div class="transcript-empty">no transcript yet</div>
  {/if}
  {#each transcript.entries as entry, i (i)}
    <div class="msg" class:user={entry.role === 'user'}>
      <div class="msg-meta">
        <span class="msg-role">{entry.role}</span>
        {#if entry.tool}<span class="msg-tool">{entry.tool}</span>{/if}
        {#if entry.usage}
          <span class="msg-usage">{entry.usage.input_tokens}→{entry.usage.output_tokens} tok</span>
        {/if}
      </div>
      {#if entry.text}<div class="msg-text">{entry.text}</div>{/if}
    </div>
  {/each}
  {#if transcript.skipped > 0}
    <!-- The producer's format-drift alarm: session-log lines it could not
         parse. A climbing number here means the CLI changed its log shape. -->
    <div class="transcript-skipped">{transcript.skipped} log line{transcript.skipped === 1 ? '' : 's'} unparsed</div>
  {/if}
</div>

<style>
  .transcript {
    flex: 1 1 auto;
    min-height: 0;
    overflow-y: auto;
    display: flex;
    flex-direction: column;
    gap: 10px;
    padding: 14px clamp(16px, 2.4vw, 24px);
  }
  .transcript-empty {
    color: var(--ink-faint);
    font-family: var(--mono);
    font-size: 12px;
    font-style: italic;
  }
  /* One message: a hairline card, the human's asks pulled right and accented so
     the turn structure reads at a glance without bubble chrome. */
  .msg {
    max-width: min(72ch, 88%);
    align-self: flex-start;
    border: 1px solid var(--edge);
    background: var(--elev, var(--panel));
    padding: 8px 12px;
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  .msg.user {
    align-self: flex-end;
    border-color: var(--accent);
    box-shadow: inset 2px 0 0 0 var(--accent);
  }
  .msg-meta {
    display: flex;
    align-items: baseline;
    gap: 8px;
    font-family: var(--mono);
    font-size: 10.5px;
    color: var(--ink-dim);
  }
  .msg-role {
    text-transform: uppercase;
    letter-spacing: 0.06em;
  }
  .msg.user .msg-role {
    color: var(--accent);
  }
  .msg-tool {
    color: var(--k-blue);
  }
  .msg-tool::before {
    content: '⚒ ';
  }
  .msg-usage {
    margin-left: auto;
    color: var(--ink-faint);
    font-variant-numeric: tabular-nums;
  }
  .msg-text {
    font-family: var(--ui);
    font-size: 13px;
    line-height: 1.45;
    color: var(--ink);
    white-space: pre-wrap;
    overflow-wrap: anywhere;
  }
  .transcript-skipped {
    align-self: center;
    font-family: var(--mono);
    font-size: 10.5px;
    color: var(--k-amber);
  }
</style>
