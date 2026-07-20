<script lang="ts">
  import SessionView from '$components/SessionView.svelte';
  import TabBar from '$components/TabBar.svelte';
  import { createSession, deleteSession, renameSession } from '$lib/api';
  import { SessionsStore } from '$lib/sessions.svelte';
  import { TermConnection } from '$lib/term.svelte';

  const store = new SessionsStore();

  let activeId = $state<string | null>(null);
  let conn = $state<TermConnection | null>(null);

  $effect(() => {
    store.start();
    return () => {
      store.dispose();
    };
  });

  // Keep the selection valid: fall back to the first session when the active
  // one disappears (or nothing is selected yet).
  $effect(() => {
    const list = store.sessions;
    if (activeId !== null && list.some((s) => s.id === activeId)) {
      return;
    }
    activeId = list.length > 0 ? list[0].id : null;
  });

  // One live terminal socket: the active session's.
  $effect(() => {
    const id = activeId;
    if (id === null) {
      conn = null;
      return;
    }
    const next = new TermConnection(id);
    conn = next;
    return () => {
      next.dispose();
    };
  });

  async function create(): Promise<void> {
    const meta = await createSession();
    activeId = meta.id;
  }

  const active = $derived(store.sessions.find((s) => s.id === activeId) ?? null);
</script>

<div class="app">
  <TabBar
    sessions={store.sessions}
    {activeId}
    onselect={(id: string) => {
      activeId = id;
    }}
    oncreate={() => {
      void create();
    }}
    onrename={(id: string, name: string) => {
      void renameSession(id, name);
    }}
    onclose={(id: string) => {
      void deleteSession(id);
    }}
  />
  <main class="main">
    {#if conn !== null && active !== null}
      {#key conn.sessionId}
        <SessionView {conn} name={active.name} />
      {/key}
    {:else}
      <div class="empty">
        <p>no sessions yet</p>
        <button
          onclick={() => {
            void create();
          }}>new session</button
        >
      </div>
    {/if}
  </main>
</div>

<style>
  .app {
    display: flex;
    height: 100vh;
  }

  .main {
    flex: 1;
    min-width: 0;
    display: flex;
  }

  .empty {
    flex: 1;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 12px;
    color: #888;
  }

  .empty button {
    background: #222;
    color: #ddd;
    border: 1px solid #333;
    border-radius: 4px;
    padding: 6px 14px;
    cursor: pointer;
  }

  .empty button:hover {
    background: #2a2a2a;
  }
</style>
