<script lang="ts">
  // The compose box: a DRAFT SHARED IN THE DOCUMENT, plus Enter to send.
  //
  // The textarea is two-way bound to the pane's `compose` LoroText (the hub
  // declares one beside every terminal pane). Local keystrokes diff into the
  // text container through `setNoteText` and travel the same /apply path as any
  // other viewer edit; remote keystrokes arrive as document frames and land in
  // `store.inputs`. Two people typing at once merge by construction -- each sees
  // the other typing -- because the draft is one mergeable text, not a value.
  //
  // Enter submits: an LWW write of `{id, text}` JSON at the pane's `send` field
  // (the uuid makes producer replay idempotent, tui::publish::deliver_sends),
  // then the shared draft is cleared for everyone. Shift+Enter is a newline.
  import { store, edits, setInput, setNoteText, canEdit, answeredBy } from '$lib/stream.svelte';
  import { peerBadge } from '$lib/peers';
  import { ui, shortAge } from '$lib/ui.svelte';
  import { inputKey } from '$lib/agent';

  let { scope, pane }: { scope: string; pane: string } = $props();

  let area: HTMLTextAreaElement | undefined = $state();

  const composeKey = $derived(inputKey(scope, pane, 'compose'));
  const sendKey = $derived(inputKey(scope, pane, 'send'));
  // The shared draft as the rendered document has it. A string when the hub has
  // declared the field (LoroText projects to a string in toJSON); undefined
  // against a producer from before compose existed, which disables the box.
  const remote = $derived(store.inputs[composeKey]);
  const draft = $derived(typeof remote === 'string' ? remote : '');
  const declared = $derived(typeof remote === 'string');
  const writable = $derived(canEdit() && declared);

  // Who sent the LAST send, from the edit history -- the cheap attribution the
  // document exposes. (Per-message attribution inside the transcript would need
  // the producer to echo the send id into the session log; it does not, so the
  // transcript rows stay unattributed.)
  const sent = $derived(answeredBy(sendKey));
  const who = $derived(sent ? peerBadge(store.peers, sent.peer, edits.localPeer) : null);

  // Push remote draft changes into the textarea, preserving the local cursor.
  // The value round-trips through the document on local typing (oninput commits,
  // renderLive reads it back), so `area.value === draft` is the steady state and
  // this only fires for edits that originated elsewhere.
  $effect(() => {
    const value = draft;
    const el = area;
    if (!el || el.value === value) return;
    const old = el.value;
    const start = el.selectionStart ?? old.length;
    // Where did the remote edit land relative to the cursor? A shared prefix
    // shorter than the cursor offset means it landed before the cursor, so the
    // cursor keeps its distance from the END of the text; otherwise it keeps
    // its offset from the start.
    let prefix = 0;
    while (prefix < old.length && prefix < value.length && old[prefix] === value[prefix]) prefix++;
    const caret = start <= prefix ? start : Math.max(prefix, value.length - (old.length - start));
    el.value = value;
    if (document.activeElement === el) el.setSelectionRange(caret, caret);
  });

  function onInput(): void {
    const el = area;
    if (!el) return;
    if (!setNoteText(composeKey, el.value)) {
      // The document refused (scrubbing, or an undeclared draft): put the
      // textarea back in sync with it rather than showing text nobody shares.
      el.value = draft;
    }
  }

  function onKeydown(event: KeyboardEvent): void {
    if (event.key !== 'Enter' || event.shiftKey) return;
    event.preventDefault();
    const text = area?.value ?? '';
    if (!text.trim() || !writable) return;
    if (!setInput(sendKey, JSON.stringify({ id: crypto.randomUUID(), text }))) return;
    setNoteText(composeKey, '');
  }
</script>

<div class="compose">
  <textarea
    bind:this={area}
    class="compose-input"
    rows="2"
    placeholder={declared ? 'message the agent (Enter sends, Shift+Enter newline)' : 'this producer has no shared draft'}
    disabled={!writable}
    value={draft}
    oninput={onInput}
    onkeydown={onKeydown}
  ></textarea>
  <div class="compose-foot">
    <span class="compose-hint">shared draft: everyone sees this as you type</span>
    {#if sent && who}
      <span class="compose-sent">
        last send by
        <span class="compose-by" class:you={who.you} class:agent={who.kind === 'agent'}>{who.label}</span>
        {shortAge(sent.ts, ui.clock)}
      </span>
    {/if}
    {#if !canEdit()}
      <span class="compose-locked">read-only: viewing an earlier version</span>
    {/if}
  </div>
</div>

<style>
  .compose {
    flex: none;
    display: flex;
    flex-direction: column;
    gap: 6px;
    padding: 10px clamp(16px, 2.4vw, 24px) 12px;
    border-top: 1px solid var(--edge);
    background: var(--panel);
  }
  .compose-input {
    resize: none;
    font-family: var(--ui);
    font-size: 13px;
    line-height: 1.45;
    color: var(--ink);
    background: var(--elev, var(--panel));
    border: 1px solid var(--edge-strong);
    padding: 8px 10px;
  }
  .compose-input:focus {
    outline: none;
    border-color: var(--accent);
  }
  .compose-input:disabled {
    color: var(--ink-dim);
    cursor: default;
  }
  .compose-foot {
    display: flex;
    align-items: baseline;
    gap: 10px;
    font-family: var(--mono);
    font-size: 10.5px;
    color: var(--ink-faint);
  }
  .compose-sent {
    color: var(--ink-dim);
  }
  .compose-by.you {
    color: var(--accent);
  }
  .compose-by.agent {
    color: var(--k-blue);
  }
  .compose-locked {
    margin-left: auto;
  }
</style>
