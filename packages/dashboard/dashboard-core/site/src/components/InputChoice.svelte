<script lang="ts">
  // A choice whose ANSWER LIVES IN THE DOCUMENT.
  //
  // Clicking writes the chosen value at this input's key in the `inputs` root
  // LoroMap, and that is the entire mechanism: no event, no request/response, no
  // payload schema. The write merges into the shared document, streams to every
  // other viewer through the same `/events` fan-out a producer's tick uses, and
  // the producer reads it back off its own document. Because a Loro map key is
  // last-write-wins, the same click twice is the same document -- idempotent by
  // construction rather than by a de-duplication rule someone has to maintain --
  // and two viewers answering at once converge on one value instead of racing.
  //
  // A producer publishes one by publishing a `data` pane with `renderer:'input'`
  // and a JSON body:
  //
  //   { "prompt": "Splice the branch?",
  //     "options": [{"value": "splice", "label": "Splice"},
  //                 {"value": "hold",   "label": "Hold", "tone": "danger"}] }
  //
  // `options` may also be bare strings, and may be omitted entirely for a plain
  // approve button. `key` overrides the input key, which otherwise defaults to the
  // pane's own document key so one input pane holds one answer.
  import { store, edits, setInput, canEdit, answeredBy } from '$lib/stream.svelte';
  import { ui, shortAge } from '$lib/ui.svelte';
  import { peerBadge } from '$lib/peers';
  import type { Pane } from '$lib/types';

  let { pane }: { pane: Pane } = $props();

  interface Option {
    value: string;
    label: string;
    danger: boolean;
  }

  const spec = $derived.by<{ key: string; prompt: string; options: Option[] }>(() => {
    let parsed: unknown = null;
    try {
      parsed = JSON.parse(pane.body ?? '');
    } catch {
      // A malformed body still renders as an approve button rather than nothing:
      // the pane exists because a producer is waiting on an answer.
    }
    const raw = (parsed && typeof parsed === 'object' ? parsed : {}) as {
      key?: unknown;
      prompt?: unknown;
      options?: unknown;
    };
    const options: Option[] = [];
    if (Array.isArray(raw.options)) {
      for (const entry of raw.options) {
        if (typeof entry === 'string') {
          options.push({ value: entry, label: entry, danger: false });
        } else if (entry && typeof entry === 'object') {
          const o = entry as { value?: unknown; label?: unknown; tone?: unknown };
          if (typeof o.value !== 'string') continue;
          options.push({
            value: o.value,
            label: typeof o.label === 'string' ? o.label : o.value,
            danger: o.tone === 'danger',
          });
        }
      }
    }
    if (!options.length) options.push({ value: 'approved', label: 'Approve', danger: false });
    return {
      key: typeof raw.key === 'string' && raw.key ? raw.key : pane.key,
      prompt: typeof raw.prompt === 'string' ? raw.prompt : (pane.title ?? ''),
      options,
    };
  });

  // The answer as it stands at the version currently rendered — so scrubbing back
  // shows what had been decided then, not what is decided now.
  const answer = $derived(store.inputs[spec.key]);
  const answered = $derived(typeof answer === 'string' ? answer : null);

  // Who decided, from the edit history. Absent when the answer predates the
  // history window, in which case the row simply says when, not who.
  const decided = $derived(answeredBy(spec.key));
  const who = $derived(decided ? peerBadge(store.peers, decided.peer, edits.localPeer) : null);

  const writable = $derived(canEdit());

  function choose(value: string): void {
    setInput(spec.key, value);
  }
</script>

<div class="input">
  {#if spec.prompt}<div class="input-prompt">{spec.prompt}</div>{/if}

  <div class="input-options" data-edit-mark={`${pane.key}|`}>
    {#each spec.options as option (option.value)}
      <button
        class="input-option"
        class:chosen={answered === option.value}
        class:danger={option.danger}
        disabled={!writable}
        onclick={() => choose(option.value)}
      >{option.label}</button>
    {/each}
  </div>

  <div class="input-foot">
    {#if answered !== null}
      <span class="input-answer">{answered}</span>
      {#if who}
        <span class="input-by" class:agent={who.kind === 'agent'} class:you={who.you}>{who.label}</span>
      {/if}
      {#if decided}<span class="input-when">{shortAge(decided.ts, ui.clock)}</span>{/if}
    {:else}
      <span class="input-waiting">waiting for an answer</span>
    {/if}
    {#if !writable}
      <span class="input-locked">read-only · viewing an earlier version</span>
    {/if}
  </div>
</div>

<style>
  .input {
    display: flex;
    flex-direction: column;
    gap: 8px;
    padding: 12px;
  }
  .input-prompt {
    font-family: var(--mono);
    font-size: 12px;
    color: var(--ink);
  }
  .input-options {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    padding: 2px;
    margin: -2px;
  }
  /* Square, hairline, flat. A chosen option is DRAWN — accent fill plus an accent
     rule — and the ones not chosen keep their normal ink: nothing is dimmed. */
  .input-option {
    font-family: var(--mono);
    font-size: 11.5px;
    color: var(--ink);
    background: var(--elev, var(--panel));
    border: 1px solid var(--edge-strong);
    padding: 4px 12px;
    cursor: pointer;
  }
  .input-option:hover:not(:disabled) {
    background: var(--sel);
  }
  .input-option:disabled {
    cursor: default;
    color: var(--ink-dim);
  }
  .input-option.chosen {
    background: var(--accent-soft);
    border-color: var(--accent);
    box-shadow: inset 2px 0 0 0 var(--accent);
  }
  .input-option.danger {
    color: var(--dead);
    border-color: color-mix(in srgb, var(--dead) 45%, var(--edge-strong));
  }
  .input-foot {
    display: flex;
    align-items: baseline;
    gap: 8px;
    font-family: var(--mono);
    font-size: 10.5px;
    color: var(--ink-dim);
  }
  .input-answer {
    color: var(--ink);
  }
  .input-by {
    color: var(--ink-dim);
  }
  .input-by.agent {
    color: var(--k-blue);
  }
  .input-by.you {
    color: var(--accent);
  }
  .input-when {
    color: var(--ink-faint);
    font-variant-numeric: tabular-nums;
  }
  .input-waiting {
    color: var(--ink-dim);
  }
  .input-locked {
    margin-left: auto;
    color: var(--ink-faint);
  }
</style>
