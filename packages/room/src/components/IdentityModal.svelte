<script lang="ts">
  // Compact identity editor. Reached from the command palette (⌘K →
  // "Set Identity…") and from the "me" segment on the status bar.
  //
  // Two modes, picked by the "GitHub user" toggle:
  //   - off → kind: 'anon', avatar = identicon SVG derived from name
  //   - on  → kind: 'github', avatar = github.com/<name>.png
  //
  // The input is the same field in both modes (display name doubles as
  // the GitHub handle when the toggle is on) — easier on the eye than
  // splitting into two stacked inputs.

  import { tick } from 'svelte';
  import { identityOpen, closeIdentity } from '$lib/ui';
  import {
    isValidGithubHandle,
    loadIdentity,
    setIdentity,
    type Identity
  } from '$lib/identity';
  import { activeRoomStores } from '$lib/store';
  import Avatar from './Avatar.svelte';

  let open = $state(false);
  $effect(() => identityOpen.subscribe((v) => (open = v)));

  let nameDraft = $state('');
  let useGithub = $state(false);
  let inputEl: HTMLInputElement | undefined = $state();

  // Reset the form whenever the modal opens so it always reflects the
  // current persisted identity, never a stale edit.
  $effect(() => {
    if (!open) return;
    const id = loadIdentity();
    nameDraft = id.kind === 'github' ? id.github ?? id.name : id.name;
    useGithub = id.kind === 'github';
    void tick().then(() => {
      inputEl?.focus();
      inputEl?.select();
    });
  });

  let trimmed = $derived(nameDraft.trim());
  let invalidGithub = $derived(useGithub && trimmed.length > 0 && !isValidGithubHandle(trimmed));
  let canSave = $derived(trimmed.length > 0 && !invalidGithub);

  // Live preview so the user sees what their new avatar will look
  // like before committing.
  let previewIdentity = $derived<Identity>({
    id: 'preview',
    name: trimmed || 'preview',
    kind: useGithub ? 'github' : 'anon',
    github: useGithub ? trimmed.toLowerCase() : undefined
  });

  function save() {
    if (!canSave) return;
    const next = setIdentity({
      name: trimmed,
      kind: useGithub ? 'github' : 'anon',
      github: useGithub ? trimmed.toLowerCase() : undefined
    });
    // Push the new identity straight into presence so peers see the
    // rename + avatar swap without waiting for the next setSelf hop.
    for (const store of activeRoomStores()) {
      store.doc.setSelf(next, { name: next.name, online: true });
    }
    closeIdentity();
  }

  function onKey(e: KeyboardEvent) {
    if (e.key === 'Escape') {
      e.preventDefault();
      e.stopPropagation();
      closeIdentity();
      return;
    }
    if (e.key === 'Enter') {
      e.preventDefault();
      save();
    }
  }
</script>

{#if open}
  <!-- svelte-ignore a11y_click_events_have_key_events -->
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div class="backdrop" onclick={closeIdentity}>
    <!-- svelte-ignore a11y_click_events_have_key_events -->
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div class="modal" onclick={(e) => e.stopPropagation()} role="dialog" aria-label="Set identity" tabindex="-1">
      <div class="head">
        <Avatar
          name={previewIdentity.name}
          github={previewIdentity.github ?? null}
          size={40}
        />
        <div class="head-text">
          <div class="head-title">Identity</div>
          <div class="head-hint">
            {useGithub ? 'GitHub avatar' : 'Anonymous identicon'}
          </div>
        </div>
      </div>

      <input
        bind:this={inputEl}
        bind:value={nameDraft}
        onkeydown={onKey}
        class="input"
        placeholder={useGithub ? 'GitHub username' : 'Display name'}
        spellcheck="false"
        autocomplete="off"
        autocapitalize="off"
        autocorrect="off"
        maxlength={39}
      />
      {#if invalidGithub}
        <p class="error">Not a valid GitHub username.</p>
      {/if}

      <label class="toggle">
        <input type="checkbox" bind:checked={useGithub} />
        <span>Use GitHub avatar</span>
      </label>

      <div class="actions">
        <button type="button" class="btn ghost" onclick={closeIdentity}>Cancel</button>
        <button type="button" class="btn primary" disabled={!canSave} onclick={save}>Save</button>
      </div>
    </div>
  </div>
{/if}

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    z-index: 1000;
    background: rgba(0, 0, 0, 0.3);
    backdrop-filter: blur(6px);
    -webkit-backdrop-filter: blur(6px);
    display: flex;
    align-items: flex-start;
    justify-content: center;
    padding-top: 16vh;
  }
  .modal {
    width: 380px;
    max-width: calc(100vw - 32px);
    background: var(--bg-elev);
    border-radius: var(--radius-lg);
    box-shadow: var(--shadow-popover);
    padding: 16px;
    display: flex;
    flex-direction: column;
    gap: 12px;
  }
  .head {
    display: flex;
    align-items: center;
    gap: 12px;
  }
  .head-text {
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .head-title {
    color: var(--text-strong);
    font-weight: 600;
    font-size: 14px;
  }
  .head-hint {
    color: var(--text-dim);
    font-size: 11.5px;
  }
  .input {
    background: var(--bg-pane);
    border: 1px solid var(--border);
    border-radius: 6px;
    padding: 8px 10px;
    color: var(--text-strong);
    font-size: 13px;
    font-family: inherit;
    outline: none;
  }
  .input:focus {
    border-color: var(--border-hi);
  }
  .toggle {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    color: var(--text);
    font-size: 12.5px;
    cursor: pointer;
    user-select: none;
  }
  .toggle input[type='checkbox'] {
    margin: 0;
  }
  .actions {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
  }
  .btn {
    padding: 6px 12px;
    border-radius: 6px;
    border: 1px solid var(--border);
    background: var(--bg-elev);
    color: var(--text);
    font-size: 12.5px;
    cursor: pointer;
  }
  .btn.primary {
    background: var(--accent, var(--text));
    color: var(--accent-text, var(--bg-pane));
    border-color: transparent;
  }
  .btn.ghost {
    background: transparent;
  }
  .btn:disabled {
    opacity: 0.55;
    cursor: default;
  }
  .error {
    margin: 0;
    color: var(--danger, #c33);
    font-size: 11.5px;
  }
</style>
