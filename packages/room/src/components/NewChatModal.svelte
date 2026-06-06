<script lang="ts">
  import { newChatOpen, closeNewChat } from '$lib/ui';
  import { roomServers, type RoomServer } from '$lib/backend';
  import { createDraft } from '$lib/drafts';
  import { router } from '$lib/router';

  let open = $state(false);
  let servers = $state<RoomServer[]>([]);
  $effect(() => newChatOpen.subscribe((v) => (open = v)));
  // Managed backends are Symphony's per-run room servers: ephemeral,
  // read-only transcript views with no live WebTransport. You can't
  // start a new chat against one, and listing every active run here
  // overflows the picker, so the New Chat list is user servers only.
  // Their threads still appear in the unified sidebar.
  $effect(() =>
    roomServers.subscribe((v) => (servers = v.filter((s) => s.enabled && !s.managed)))
  );

  function choose(server: RoomServer) {
    const id = createDraft(server.id);
    closeNewChat();
    router.go('/s/' + encodeURIComponent(server.id) + '/t/' + encodeURIComponent(id));
  }
</script>

{#if open}
  <!-- svelte-ignore a11y_click_events_have_key_events -->
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div class="backdrop" onclick={closeNewChat}>
    <!-- svelte-ignore a11y_click_events_have_key_events -->
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div
      class="modal"
      role="dialog"
      aria-label="Choose server"
      tabindex="-1"
      onclick={(e) => e.stopPropagation()}
    >
      <div class="head">
        <div class="title">New Chat</div>
        <div class="sub">Choose a room server.</div>
      </div>

      <div class="servers">
        {#if servers.length === 0}
          <div class="empty">No enabled servers.</div>
        {:else}
          {#each servers as server (server.id)}
            <button type="button" class="server" onclick={() => choose(server)}>
              <span class="name">{server.name}</span>
              <span class="url">{server.httpBase || 'current origin'}</span>
            </button>
          {/each}
        {/if}
      </div>

      <div class="actions">
        <button type="button" class="btn" onclick={closeNewChat}>Cancel</button>
      </div>
    </div>
  </div>
{/if}

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    z-index: 1000;
    background: rgba(0, 0, 0, 0.28);
    backdrop-filter: blur(6px);
    -webkit-backdrop-filter: blur(6px);
    display: flex;
    align-items: flex-start;
    justify-content: center;
    padding-top: 18vh;
  }
  .modal {
    width: 420px;
    max-width: calc(100vw - 32px);
    max-height: 64vh;
    background: var(--bg-elev);
    border-radius: var(--radius-lg);
    box-shadow: var(--shadow-popover);
    border: 1px solid var(--border-hi);
    padding: 16px;
    display: flex;
    flex-direction: column;
    gap: 14px;
  }
  .title {
    color: var(--text-strong);
    font-weight: 650;
    font-size: 14px;
  }
  .sub {
    margin-top: 3px;
    color: var(--text-dim);
    font-size: 12px;
  }
  .servers {
    display: flex;
    flex-direction: column;
    gap: 8px;
    min-height: 0;
    overflow-y: auto;
  }
  .server {
    display: grid;
    gap: 3px;
    width: 100%;
    text-align: left;
    padding: 10px 11px;
    border-radius: var(--radius);
    border: 1px solid var(--border);
    background: var(--bg-pane);
    color: var(--text);
  }
  .server:hover {
    border-color: var(--accent);
    background: var(--bg-hover);
  }
  .name {
    font-size: 13px;
    font-weight: 600;
    color: var(--text-strong);
  }
  .url {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--text-dim);
    font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
    font-size: 11px;
  }
  .empty {
    color: var(--text-dim);
    font-size: 12px;
  }
  .actions {
    display: flex;
    justify-content: flex-end;
  }
  .btn {
    border: 1px solid var(--border);
    background: var(--bg-pane);
    color: var(--text);
    border-radius: var(--radius-sm);
    padding: 5px 10px;
    font-size: 12px;
  }
</style>
