<script lang="ts">
  // Reviewer note for one agent-side message.
  //
  // Renders a small flag affordance that opens a popover with a free-
  // text field. Notes ride the room's Loro doc as a per-message map
  // (see `annotationsFor` in `$lib/loro`) so concurrent reviewers
  // don't race each other, and the server mirrors them into SQL so
  // we can later mine the corpus to improve AGENTS.md.

  import { untrack } from 'svelte';
  import IconFlag from '~icons/ph/flag';
  import IconFlagFill from '~icons/ph/flag-fill';
  import IconX from '~icons/ph/x';
  import type { Annotation } from '$lib/loro';
  import { roomFor } from '$lib/store';
  import { loadIdentity } from '$lib/identity';
  import { humanAgo, absoluteTime } from '$lib/time';
  import { nowTick } from '$lib/activity';

  interface Props {
    serverId: string;
    messageId: string;
  }

  let { serverId, messageId }: Props = $props();
  const roomDoc = untrack(() => roomFor(serverId).doc);

  // One MessageAnnotations handle per message id, shared across every
  // remount of this component for the same id (the cache lives in
  // roomDoc), so the Loro subscription stays attached. `untrack`
  // silences Svelte 5's once-at-mount prop-tracking warning — the
  // component is always remounted when the parent keys on message id.
  const handle = untrack(() => roomDoc.annotationsFor(messageId));
  let entries = $state<Annotation[]>(handle.current());
  $effect(() => handle.list.subscribe((next) => (entries = next)));

  let now = $state(Date.now());
  const unsubNow = nowTick.subscribe((v) => (now = v));
  $effect(() => () => unsubNow());

  let open = $state(false);
  let draft = $state('');
  let textareaEl: HTMLTextAreaElement | null = $state(null);

  $effect(() => {
    if (open && textareaEl) textareaEl.focus();
  });

  function toggle() {
    open = !open;
  }

  function submit() {
    const text = draft.trim();
    if (text.length === 0) return;
    handle.add(loadIdentity(), text);
    draft = '';
  }

  function onKey(ev: KeyboardEvent) {
    if (ev.key === 'Escape') {
      open = false;
      ev.preventDefault();
      return;
    }
    if ((ev.metaKey || ev.ctrlKey) && ev.key === 'Enter') {
      submit();
      ev.preventDefault();
    }
  }

  function remove(id: string) {
    handle.remove(id);
  }
</script>

<div class="anno" class:has-notes={entries.length > 0} class:open>
  <button
    type="button"
    class="trigger"
    onclick={toggle}
    aria-label={entries.length > 0
      ? `${entries.length} reviewer note${entries.length === 1 ? '' : 's'}`
      : 'Flag this turn as needing improvement'}
    aria-expanded={open}
  >
    {#if entries.length > 0}
      <IconFlagFill width={12} height={12} />
      <span class="count">{entries.length}</span>
    {:else}
      <IconFlag width={12} height={12} />
    {/if}
  </button>

  {#if open}
    <!-- Anchored popover. Click-outside closes via the backdrop, not a
         document listener — the backdrop lives in the same DOM subtree
         so we don't have to special-case the trigger button. -->
    <button
      type="button"
      class="backdrop"
      aria-label="Close"
      onclick={() => (open = false)}
    ></button>
    <div class="popover" role="dialog" aria-label="Reviewer notes">
      {#if entries.length > 0}
        <ul class="entries">
          {#each entries as a (a.id)}
            <li class="entry">
              <div class="meta">
                <span class="author">{a.author_name}</span>
                <span class="when" title={absoluteTime(a.ts_ms)}
                  >{humanAgo(a.ts_ms, now)}</span
                >
                <button
                  type="button"
                  class="remove"
                  onclick={() => remove(a.id)}
                  aria-label="Remove note"
                  title="Remove note"
                >
                  <IconX width={11} height={11} />
                </button>
              </div>
              <div class="body">{a.text}</div>
            </li>
          {/each}
        </ul>
      {/if}
      <div class="composer">
        <textarea
          bind:this={textareaEl}
          bind:value={draft}
          onkeydown={onKey}
          rows="3"
          placeholder="Why does this turn need improvement?"
        ></textarea>
        <div class="actions">
          <span class="hint">⌘↵ to save</span>
          <button type="button" class="save" onclick={submit} disabled={draft.trim().length === 0}
            >Add note</button
          >
        </div>
      </div>
    </div>
  {/if}
</div>

<style>
  .anno {
    position: relative;
    display: inline-flex;
    align-items: center;
  }
  .trigger {
    display: inline-flex;
    align-items: center;
    gap: 3px;
    padding: 2px 5px;
    border-radius: 4px;
    color: var(--text-dim);
    opacity: 0;
    transition: opacity 0.12s, color 0.12s, background 0.12s;
    font-size: 11px;
    line-height: 1;
  }
  .trigger:hover {
    color: var(--text);
    background: var(--bg-hover);
  }
  .anno.has-notes .trigger,
  .anno.open .trigger,
  :global([data-message-id]:hover) .trigger,
  :global(.step:hover) .trigger,
  :global(.row:hover) .trigger {
    opacity: 1;
  }
  .anno.has-notes .trigger {
    color: var(--accent, #d97706);
  }
  .count {
    font-variant-numeric: tabular-nums;
    font-weight: 500;
  }

  .backdrop {
    position: fixed;
    inset: 0;
    background: transparent;
    border: 0;
    cursor: default;
    z-index: 50;
  }
  .popover {
    position: absolute;
    top: 100%;
    right: 0;
    margin-top: 4px;
    width: 320px;
    max-height: 360px;
    overflow-y: auto;
    background: var(--bg-elev, var(--bg));
    border: 1px solid var(--border-hi, var(--border));
    border-radius: 8px;
    box-shadow: 0 8px 22px rgba(0, 0, 0, 0.22);
    padding: 8px;
    z-index: 51;
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  .entries {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .entry {
    background: var(--bg-pill);
    border: 1px solid var(--border);
    border-radius: 6px;
    padding: 6px 8px;
    display: flex;
    flex-direction: column;
    gap: 3px;
  }
  .meta {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: 11px;
    color: var(--text-dim);
  }
  .author {
    color: var(--text);
    font-weight: 500;
  }
  .when {
    font-variant-numeric: tabular-nums;
  }
  .remove {
    margin-left: auto;
    display: inline-flex;
    color: var(--text-dim);
    padding: 2px;
    border-radius: 4px;
  }
  .remove:hover {
    color: var(--text);
    background: var(--bg-hover);
  }
  .body {
    color: var(--text);
    font-size: 12px;
    line-height: 1.45;
    white-space: pre-wrap;
    overflow-wrap: anywhere;
  }

  .composer {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  textarea {
    resize: vertical;
    min-height: 60px;
    max-height: 200px;
    padding: 6px 8px;
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: 6px;
    color: var(--text);
    font-family: inherit;
    font-size: 12.5px;
    line-height: 1.45;
  }
  textarea:focus {
    outline: none;
    border-color: var(--border-hi, var(--border));
  }
  .actions {
    display: flex;
    align-items: center;
    justify-content: space-between;
  }
  .hint {
    font-size: 10.5px;
    color: var(--text-dim);
  }
  .save {
    background: var(--accent, var(--text-strong));
    color: var(--bg);
    padding: 4px 10px;
    border-radius: 6px;
    font-size: 11.5px;
    font-weight: 500;
  }
  .save:disabled {
    opacity: 0.45;
    cursor: not-allowed;
  }
</style>
