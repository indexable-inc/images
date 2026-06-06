<script lang="ts">
  // Settings overlay. Opens from the sidebar Settings button (and the
  // ⌘, accelerator via lib/commands.ts). Sits at this top level — and
  // not inline under the sidebar — because the rows here carry full
  // URLs and peer lists that don't fit a 200px-wide bay without
  // wrapping into something illegible.

  import { tick } from 'svelte';
  import { settingsOpen, closeSettings } from '$lib/ui';
  import { isValidGithubHandle, loadIdentity, setIdentity } from '$lib/identity';
  import type { PresenceEntry } from '$lib/loro';
  import {
    roomServers,
    upsertRoomServer,
    removeRoomServer,
    setRoomServerEnabled,
    type RoomServer
  } from '$lib/backend';
  import { activeRoomStores } from '$lib/store';
  import { durationClock } from '$lib/time';
  import { nowTick, isIdle, peerLiveness } from '$lib/activity';
  import Avatar from './Avatar.svelte';
  import ZzzIndicator from './ZzzIndicator.svelte';

  let open = $state(false);
  $effect(() => settingsOpen.subscribe((v) => (open = v)));

  let identity = $state(loadIdentity());
  let editingName = $state(false);
  let nameDraft = $state('');
  let useGithub = $state(false);
  let nameInputEl = $state<HTMLInputElement | undefined>();

  let nameTrimmed = $derived(nameDraft.trim());
  let invalidGithub = $derived(
    useGithub && nameTrimmed.length > 0 && !isValidGithubHandle(nameTrimmed)
  );

  let servers = $state<RoomServer[]>([]);
  let editingServer = $state(false);
  let editingServerId = $state<string | null>(null);
  let serverNameDraft = $state('');
  let serverDraft = $state('');
  let serverError = $state<string | null>(null);
  let serverInputEl = $state<HTMLInputElement | undefined>();

  // Refresh on open so the form never shows a stale edit. Read the
  // world into LOCAL variables first and assign once into $state —
  // re-reading the same $state slot after writing it in the same
  // effect would register it as a dep, the effect's next tick would
  // re-fire on that write, loadIdentity() would mint a fresh object
  // reference each pass, and Svelte would tear the page down with
  // effect_update_depth_exceeded.
  $effect(() => {
    if (!open) return;
    const id = loadIdentity();
    identity = id;
    editingName = false;
    nameDraft = '';
    useGithub = id.kind === 'github';
    editingServer = false;
    editingServerId = null;
    serverNameDraft = '';
    serverDraft = '';
    serverError = null;
  });

  $effect(() => roomServers.subscribe((v) => (servers = v)));

  // User-defined servers carry the editable controls. Managed backends
  // are Symphony's per-run room servers: discovered from /api/backends,
  // ephemeral, and read-only here (editing or removing one just races
  // the next registry refresh), so they render as a compact, scrollable
  // reference list rather than editable rows.
  let userServers = $derived(servers.filter((s) => !s.managed));
  let managedServers = $derived(servers.filter((s) => s.managed));

  let presence = $state<PresenceEntry[]>([]);
  $effect(() => {
    void servers;
    const stores = activeRoomStores();
    if (stores.length === 0) {
      presence = [];
      return;
    }
    const snapshots = new Map<string, PresenceEntry[]>();
    const unsubs = stores.map((store) =>
      store.doc.presenceList.subscribe((v) => {
        snapshots.set(store.server.id, v);
        presence = [...snapshots.values()].flat();
      })
    );
    return () => unsubs.forEach((unsub) => unsub());
  });

  let now = $state(Date.now());
  $effect(() => nowTick.subscribe((v) => (now = v)));

  // Annotate each candidate peer with its liveness state so the
  // template can render 'live' and 'dying' differently without
  // re-calling peerLiveness three times per row. `gone` peers are
  // filtered out — they've either explicitly disconnected or missed
  // enough heartbeats that we shouldn't pretend they're still in the
  // room.
  let livePeers = $derived(
    presence
      .filter((p) => p.id !== identity.id)
      .map((p) => ({ peer: p, liveness: peerLiveness(p, now) }))
      .filter(({ liveness }) => liveness !== 'gone')
  );

  function startEditName() {
    nameDraft =
      identity.kind === 'github' ? identity.github ?? identity.name : identity.name;
    useGithub = identity.kind === 'github';
    editingName = true;
    void tick().then(() => nameInputEl?.focus());
  }

  function commitName() {
    if (invalidGithub) return;
    const next = setIdentity({
      name: nameTrimmed,
      kind: useGithub ? 'github' : 'anon',
      github: useGithub ? nameTrimmed.toLowerCase() : undefined
    });
    identity = next;
    nameDraft = '';
    useGithub = next.kind === 'github';
    editingName = false;
    for (const store of activeRoomStores()) {
      store.doc.setSelf(next, { name: next.name, online: true });
    }
  }

  function toggleGithub() {
    if (editingName) {
      // mid-edit: just flip the flag; commit happens on Enter/blur
      useGithub = !useGithub;
      return;
    }
    // Out of edit mode: flip immediately and persist.
    const next = setIdentity({
      name: identity.name,
      kind: identity.kind === 'github' ? 'anon' : 'github',
      github:
        identity.kind === 'github'
          ? undefined
          : (identity.github ?? identity.name).toLowerCase()
    });
    identity = next;
    useGithub = next.kind === 'github';
    for (const store of activeRoomStores()) {
      store.doc.setSelf(next, { name: next.name, online: true });
    }
  }

  function startEditServer(server?: RoomServer) {
    editingServerId = server?.id ?? null;
    serverNameDraft = server?.name ?? '';
    serverDraft = server?.httpBase ?? '';
    serverError = null;
    editingServer = true;
    void tick().then(() => serverInputEl?.focus());
  }

  function commitServer() {
    try {
      upsertRoomServer({
        id: editingServerId ?? undefined,
        name: serverNameDraft,
        httpBase: serverDraft,
        enabled: true
      });
      serverError = null;
      editingServer = false;
      editingServerId = null;
      serverNameDraft = '';
      serverDraft = '';
    } catch (err) {
      serverError = err instanceof Error ? err.message : String(err);
    }
  }

  function onKey(e: KeyboardEvent) {
    // Esc closes the modal — but only when no inner edit field is
    // owning the keystroke. Inputs handle their own Escape to cancel
    // the in-progress edit, which feels less abrupt than closing the
    // whole sheet on a stray Esc mid-typing.
    if (e.key !== 'Escape') return;
    if (editingName || editingServer) return;
    e.preventDefault();
    e.stopPropagation();
    closeSettings();
  }
</script>

{#if open}
  <!-- svelte-ignore a11y_click_events_have_key_events -->
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div class="backdrop" onclick={closeSettings} onkeydown={onKey}>
    <!-- svelte-ignore a11y_click_events_have_key_events -->
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div
      class="modal"
      onclick={(e) => e.stopPropagation()}
      role="dialog"
      aria-label="Settings"
      tabindex="-1"
    >
      <div class="head">
        <div class="head-title">Settings</div>
      </div>

      <div class="rows">
        <div class="row align-top">
          <span class="row-label">Identity</span>
          <div class="identity-edit">
            {#if editingName}
              <input
                bind:this={nameInputEl}
                type="text"
                class="row-input"
                placeholder={useGithub ? 'GitHub username' : 'Display name'}
                bind:value={nameDraft}
                onblur={commitName}
                onkeydown={(e) => {
                  if (e.key === 'Enter') commitName();
                  if (e.key === 'Escape') {
                    e.stopPropagation();
                    nameDraft = '';
                    useGithub = identity.kind === 'github';
                    editingName = false;
                  }
                }}
                maxlength={39}
                autocomplete="off"
                autocapitalize="off"
                autocorrect="off"
                spellcheck="false"
              />
              {#if invalidGithub}
                <span class="row-error">Not a valid GitHub username.</span>
              {/if}
            {:else}
              <button class="name" type="button" onclick={startEditName}>
                <Avatar name={identity.name} github={identity.github ?? null} size={20} />
                <span>{identity.name}</span>
              </button>
            {/if}
            <label class="toggle">
              <input type="checkbox" checked={useGithub} onchange={toggleGithub} />
              <span>Use GitHub avatar</span>
            </label>
          </div>
        </div>

        <div class="row align-top">
          <span class="row-label">Servers</span>
          {#if editingServer}
            <div class="server-edit">
              <input
                type="text"
                class="row-input"
                placeholder="Name"
                bind:value={serverNameDraft}
                autocomplete="off"
                spellcheck="false"
              />
              <input
                bind:this={serverInputEl}
                type="url"
                class="row-input mono"
                placeholder="http://localhost:8080"
                bind:value={serverDraft}
                onblur={commitServer}
                onkeydown={(e) => {
                  if (e.key === 'Enter') commitServer();
                  if (e.key === 'Escape') {
                    e.stopPropagation();
                    editingServer = false;
                    editingServerId = null;
                    serverError = null;
                  }
                }}
                autocomplete="off"
                spellcheck="false"
              />
              {#if serverError}
                <span class="row-error">{serverError}</span>
              {/if}
            </div>
          {:else}
            <div class="server-list">
              {#each userServers as server (server.id)}
                <div class="server-line">
                  <label class="server-enabled">
                    <input
                      type="checkbox"
                      checked={server.enabled}
                      onchange={(e) =>
                        setRoomServerEnabled(
                          server.id,
                          (e.currentTarget as HTMLInputElement).checked
                        )}
                    />
                    <span class="server-main">
                      <span class="server-name">{server.name}</span>
                      <span class="server-url">{server.httpBase || 'current origin'}</span>
                    </span>
                  </label>
                  <button class="mini" type="button" onclick={() => startEditServer(server)}>edit</button>
                  <button class="mini danger" type="button" onclick={() => removeRoomServer(server.id)}>remove</button>
                </div>
              {/each}
              <button class="add-server" type="button" onclick={() => startEditServer()}>
                Add server
              </button>

              {#if managedServers.length > 0}
                <div class="managed-head">
                  Run backends
                  <span class="managed-count">{managedServers.length}</span>
                </div>
                <div class="managed-list">
                  {#each managedServers as server (server.id)}
                    <div class="managed-line">
                      <span class="server-name">
                        {server.name}
                        {#if server.runtime}
                          <span class="server-runtime">{server.runtime}</span>
                        {/if}
                      </span>
                      <span class="server-url">{server.httpBase || 'current origin'}</span>
                    </div>
                  {/each}
                </div>
              {/if}
            </div>
          {/if}
        </div>

        <div class="row align-top">
          <span class="row-label">Live now</span>
          <div class="live-list">
            {#if livePeers.length === 0}
              <span class="muted">just you</span>
            {:else}
              {#each livePeers as entry (entry.peer.id + ':' + entry.peer.last_seen_ms)}
                {@const p = entry.peer}
                {@const dying = entry.liveness === 'dying'}
                {@const idle = !dying && isIdle(p.last_active_ms, now)}
                {@const idleFor = idle ? durationClock(now - p.last_active_ms) : ''}
                <span
                  class="live-pill"
                  class:idle
                  class:dying
                  title={dying
                    ? `${p.name} — disconnecting`
                    : idle
                      ? `${p.name} — idle ${idleFor}`
                      : p.name}
                >
                  <Avatar name={p.name} github={p.github} size={14} />
                  <span class="live-name">{p.name}</span>
                  {#if idle}
                    <span class="idle-meta">
                      <ZzzIndicator size={10} label={`${p.name} idle ${idleFor}`} />
                      <span class="idle-dur">{idleFor}</span>
                    </span>
                  {/if}
                </span>
              {/each}
            {/if}
          </div>
        </div>
      </div>

      <div class="actions">
        <button type="button" class="btn ghost" onclick={closeSettings}>Close</button>
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
    padding-top: 14vh;
  }
  .modal {
    width: 480px;
    max-width: calc(100vw - 32px);
    max-height: 72vh;
    background: var(--bg-elev);
    border-radius: var(--radius-lg);
    box-shadow: var(--shadow-popover);
    padding: 18px 18px 14px;
    display: flex;
    flex-direction: column;
    gap: 14px;
  }
  .head-title {
    color: var(--text-strong);
    font-weight: 600;
    font-size: 14px;
  }
  .rows {
    display: flex;
    flex-direction: column;
    gap: 12px;
    min-height: 0;
    overflow-y: auto;
  }
  .row {
    display: flex;
    align-items: center;
    gap: 12px;
    min-width: 0;
  }
  .row.align-top {
    align-items: flex-start;
  }
  .row-label {
    color: var(--text-dim);
    font-size: 12px;
    flex-shrink: 0;
    width: 96px;
  }
  .row-input {
    flex: 1;
    background: var(--bg-pane);
    border: 1px solid var(--border);
    border-radius: 6px;
    padding: 6px 8px;
    color: var(--text-strong);
    font-size: 13px;
    font-family: inherit;
    outline: none;
  }
  .row-input.mono {
    font-family: var(--font-mono);
    font-size: 12px;
  }
  .row-input:focus {
    border-color: var(--border-hi);
  }
  .row-error {
    color: var(--danger);
    font-size: 11px;
    margin-top: 4px;
  }
  .server-edit {
    display: flex;
    flex-direction: column;
    gap: 6px;
    flex: 1;
    min-width: 0;
  }
  .identity-edit {
    display: flex;
    flex-direction: column;
    gap: 6px;
    flex: 1;
    min-width: 0;
  }
  .toggle {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    color: var(--text-muted);
    font-size: 11.5px;
    cursor: pointer;
    user-select: none;
    align-self: flex-start;
  }
  .toggle input[type='checkbox'] {
    margin: 0;
  }
  .name {
    display: inline-flex;
    align-items: center;
    gap: 8px;
    padding: 4px 8px;
    border-radius: 6px;
    color: var(--text-strong);
    font-size: 13px;
    font-weight: 500;
    cursor: pointer;
  }
  .name:hover {
    background: var(--bg-hover);
  }
  .server-list {
    display: flex;
    flex-direction: column;
    gap: 6px;
    flex: 1;
    min-width: 0;
  }
  .server-line {
    display: grid;
    grid-template-columns: minmax(0, 1fr) auto auto;
    align-items: center;
    gap: 6px;
  }
  .server-enabled {
    display: flex;
    align-items: center;
    gap: 8px;
    min-width: 0;
    padding: 4px 8px;
    border-radius: 6px;
    cursor: pointer;
  }
  .server-enabled:hover {
    background: var(--bg-hover);
  }
  .server-main {
    display: grid;
    min-width: 0;
  }
  .server-name {
    color: var(--text-strong);
    font-size: 12px;
    font-weight: 600;
  }
  .server-runtime {
    margin-left: 6px;
    padding: 0 6px;
    border-radius: 999px;
    background: var(--surface-2, rgba(127, 127, 127, 0.18));
    color: var(--text);
    font-family: var(--font-mono);
    font-size: 10px;
    font-weight: 500;
    text-transform: uppercase;
    letter-spacing: 0.04em;
  }
  .server-url {
    color: var(--text);
    font-family: var(--font-mono);
    font-size: 11px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    min-width: 0;
  }
  .mini,
  .add-server {
    border-radius: 4px;
    color: var(--text-dim);
    padding: 4px 8px;
    font-size: 11px;
  }
  .mini:hover,
  .add-server:hover {
    color: var(--text-muted);
    background: var(--bg-hover);
  }
  .mini.danger:hover {
    color: var(--danger);
  }
  .add-server {
    align-self: flex-start;
  }
  .managed-head {
    display: flex;
    align-items: center;
    gap: 6px;
    margin-top: 8px;
    color: var(--text-dim);
    font-size: 11px;
    text-transform: uppercase;
    letter-spacing: 0.04em;
  }
  .managed-count {
    padding: 0 6px;
    border-radius: 999px;
    background: var(--surface-2, rgba(127, 127, 127, 0.18));
    color: var(--text);
    font-family: var(--font-mono);
    font-size: 10px;
    letter-spacing: 0;
  }
  /* Run backends can number in the dozens on a busy Symphony host;
     cap the panel and scroll rather than pushing the modal past the
     viewport. */
  .managed-list {
    display: flex;
    flex-direction: column;
    gap: 4px;
    max-height: 168px;
    overflow-y: auto;
    padding-right: 2px;
  }
  .managed-line {
    display: grid;
    min-width: 0;
    padding: 4px 8px;
    border-radius: 6px;
    background: var(--bg-pane);
  }
  .live-list {
    display: flex;
    flex-wrap: wrap;
    gap: 4px;
    flex: 1;
    min-width: 0;
    padding-top: 2px;
  }
  .live-pill {
    position: relative;
    display: inline-flex;
    align-items: center;
    gap: 5px;
    padding: 2px 6px 2px 3px;
    background: var(--bg-pill);
    border: 1px solid var(--border);
    border-radius: 999px;
    font-size: 11px;
    color: var(--text);
    transition: opacity 0.2s ease, color 0.2s ease;
    max-width: 100%;
    min-width: 0;
  }
  .live-pill.idle {
    color: var(--text-muted);
    opacity: 0.7;
    padding-right: calc(6ch + 22px);
  }
  /* Dying: peer either explicitly went offline or has missed enough
     heartbeats that we suspect they're gone. Fade to invisible over a
     few seconds — long enough to read as "they left" instead of a
     stutter, short enough that the slot frees up before the user can
     wonder why the avatar is still there. The peer is dropped from
     livePeers entirely once peerLiveness returns 'gone', so the pill
     unmounts at the same moment the animation finishes. */
  .live-pill.dying {
    color: var(--text-dim);
    animation: peer-dying 2.6s ease-out forwards;
    pointer-events: none;
  }
  @keyframes peer-dying {
    0%   { opacity: 0.9; filter: grayscale(0.4); transform: translateY(0); }
    70%  { opacity: 0.35; filter: grayscale(1); transform: translateY(-2px); }
    100% { opacity: 0;    filter: grayscale(1); transform: translateY(-6px); }
  }
  .idle-meta {
    position: absolute;
    right: 6px;
    top: 50%;
    transform: translateY(-50%);
    display: inline-flex;
    align-items: center;
    gap: 4px;
    pointer-events: none;
  }
  .idle-dur {
    color: var(--text-dim);
    font-family: var(--font-mono);
    font-size: 10px;
    font-variant-numeric: tabular-nums;
    font-feature-settings: 'tnum';
    line-height: 1;
  }
  .live-name {
    display: inline-block;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .muted {
    color: var(--text-dim);
    font-size: 12px;
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
  .btn.ghost {
    background: transparent;
  }
</style>
