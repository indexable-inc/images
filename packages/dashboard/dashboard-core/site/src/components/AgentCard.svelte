<script lang="ts">
  // The web-native face of one agent: name/kind, a live status badge, the
  // chat-shaped transcript, and the shared compose box. This is the PRIMARY
  // surface -- how the page would look if the TUI never existed -- with the raw
  // terminal one toggle away (the full power of the tui, nothing hidden).
  //
  // Mounted by the stage for any terminal pane carrying an `agent` label; the
  // companion transcript pane (renderer:'transcript', parent = this terminal)
  // is resolved from the live pane set each frame, so it appears the moment the
  // producer finds the agent's session log.
  import { store } from '$lib/stream.svelte';
  import { paneId, withKey } from '$lib/run';
  import { transcriptKeyFor } from '$lib/agent';
  import Transcript from './Transcript.svelte';
  import Compose from './Compose.svelte';
  import TermBody from './TermBody.svelte';
  import type { Pane } from '$lib/types';

  let { pane }: { pane: Pane } = $props();

  // The raw-TUI escape hatch, per card and not persisted: the chat is the
  // resting state.
  let showTerminal = $state(false);

  const id = $derived(paneId(pane.key));
  const transcriptPane = $derived.by<Pane | null>(() => {
    const key = transcriptKeyFor(store.panes, pane.key);
    return key ? withKey(key, store.panes[key], pane.scope) : null;
  });
  // The wire's four states, with "completed" derived from a dead pane even if
  // the producer never flipped the status before exiting.
  const status = $derived(pane.alive === false ? 'completed' : (pane.status ?? ''));
</script>

<div class="agent">
  <header class="agent-head">
    <span class="agent-name">{pane.title || pane.agent}</span>
    <span class="agent-kind">{pane.agent}</span>
    {#if status}
      <span
        class="agent-status"
        class:working={status === 'working'}
        class:awaiting={status === 'awaiting_input'}
        class:gate={status === 'gate'}
        class:completed={status === 'completed'}
      >{status.replace('_', ' ')}</span>
    {/if}
    <span class="agent-spacer"></span>
    <button
      class="agent-toggle"
      class:active={showTerminal}
      onclick={() => (showTerminal = !showTerminal)}
    >{showTerminal ? 'chat' : 'terminal'}</button>
  </header>

  {#if showTerminal}
    <!-- Mirror the resource stage's `.pane > .body` wrapping so TermBody's CSS
         (scoped under `.pane`) applies here too. -->
    <div class="pane term agent-term">
      <div class="body term-body">
        <TermBody {pane} />
      </div>
    </div>
  {:else if transcriptPane}
    <Transcript pane={transcriptPane} />
  {:else}
    <div class="agent-waiting">no transcript yet: the producer publishes it once the agent's session log appears</div>
  {/if}

  <Compose scope={pane.scope} pane={id} />
</div>

<style>
  .agent {
    flex: 1 1 auto;
    min-height: 0;
    display: flex;
    flex-direction: column;
  }
  .agent-head {
    flex: none;
    display: flex;
    align-items: baseline;
    gap: 10px;
    padding: 12px clamp(16px, 2.4vw, 24px);
    border-bottom: 1px solid var(--edge);
  }
  .agent-name {
    font-family: var(--mono);
    font-size: 13px;
    font-weight: 600;
    color: var(--ink);
  }
  .agent-kind {
    font-family: var(--mono);
    font-size: 11.5px;
    color: var(--k-blue);
  }
  /* The status badge: hairline chip, colored by what the state asks of you.
     Working is amber (busy), awaiting input is the accent (your turn), a gate
     is red (blocked on an approval), completed is quiet. */
  .agent-status {
    font-family: var(--mono);
    font-size: 10.5px;
    padding: 1px 8px;
    border: 1px solid var(--edge-strong);
    color: var(--ink-dim);
  }
  .agent-status.working {
    color: var(--k-amber);
    border-color: var(--k-amber);
  }
  .agent-status.awaiting {
    color: var(--accent);
    border-color: var(--accent);
    background: var(--accent-soft);
  }
  .agent-status.gate {
    color: var(--dead);
    border-color: var(--dead);
  }
  .agent-status.completed {
    color: var(--ink-faint);
  }
  .agent-spacer {
    flex: 1 1 auto;
  }
  .agent-toggle {
    font-family: var(--mono);
    font-size: 11px;
    color: var(--ink-dim);
    background: transparent;
    border: 1px solid var(--edge-strong);
    padding: 2px 10px;
    cursor: pointer;
  }
  .agent-toggle:hover {
    background: var(--sel);
    color: var(--ink);
  }
  .agent-toggle.active {
    background: var(--accent-soft);
    border-color: var(--accent);
    color: var(--ink);
  }
  .agent-term {
    flex: 1 1 auto;
    min-height: 0;
  }
  .agent-term .body {
    flex: 1 1 auto;
    min-height: 0;
    overflow: auto;
  }
  .agent-waiting {
    flex: 1 1 auto;
    display: grid;
    place-content: center;
    color: var(--ink-faint);
    font-family: var(--mono);
    font-size: 12px;
    padding: 20px;
    text-align: center;
  }
</style>
