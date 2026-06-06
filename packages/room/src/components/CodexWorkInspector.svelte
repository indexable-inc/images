<script lang="ts">
  import type { CodexEventRecord, CodexRequestRecord, CodexWorkGraphNode } from '$lib/loro';
  import { roomFor } from '$lib/store';
  import * as api from '$lib/api';

  interface Props {
    serverId: string;
    threadId: string;
  }

  let { serverId, threadId }: Props = $props();
  let roomDoc = $derived(roomFor(serverId).doc);

  let events = $state<CodexEventRecord[]>([]);
  let requests = $state<CodexRequestRecord[]>([]);
  let graph = $state<CodexWorkGraphNode[]>([]);
  let pendingAction = $state<string | null>(null);
  let error = $state<string | null>(null);

  $effect(() => roomDoc.codexEvents.subscribe((v) => (events = v)));
  $effect(() => roomDoc.codexPendingRequests.subscribe((v) => (requests = v)));
  $effect(() => roomDoc.codexWorkGraph.subscribe((v) => (graph = v)));

  let threadEvents = $derived(events.filter((e) => e.threadId === threadId).slice(-80).reverse());
  let threadRequests = $derived(
    requests.filter((r) => !r.threadId || r.threadId === threadId)
  );
  let threadGraph = $derived(
    graph.filter(
      (n) =>
        n.senderThreadId === threadId ||
        n.receiverThreadIds.includes(threadId) ||
        n.receiverThreadIds.length > 0
    )
  );

  function shortMethod(method: string): string {
    const parts = method.split('/');
    return parts.length > 2 ? parts.slice(-2).join('/') : method;
  }

  function requestTitle(req: CodexRequestRecord): string {
    return req.method ? shortMethod(req.method) : `request ${req.requestId}`;
  }

  function eventDetail(ev: CodexEventRecord): string {
    if (ev.itemId) return ev.itemId;
    if (ev.turnId) return ev.turnId;
    return ev.threadId ?? '';
  }

  function stateLabel(value: unknown): string {
    if (!value || typeof value !== 'object') return '';
    const rec = value as Record<string, unknown>;
    if (typeof rec.status === 'string') return rec.status;
    if ('completed' in rec) return 'completed';
    if ('errored' in rec) return 'errored';
    return '';
  }

  async function decide(req: CodexRequestRecord, decision: 'accept' | 'decline') {
    pendingAction = req.requestId;
    error = null;
    try {
      await api.respondCodexRequest(serverId, req.requestId, { decision });
    } catch (err) {
      error = (err as Error).message;
    } finally {
      pendingAction = null;
    }
  }
</script>

<aside class="inspector" aria-label="Codex work inspector">
  <section class="panel approvals" class:hot={threadRequests.length > 0}>
    <header>
      <span>Approvals</span>
      <strong>{threadRequests.length}</strong>
    </header>
    {#if error}
      <p class="error">{error}</p>
    {/if}
    {#if threadRequests.length === 0}
      <p class="empty">No pending Codex requests.</p>
    {:else}
      {#each threadRequests as req (req.requestId)}
        <div class="request">
          <div class="request-main">
            <span class="method">{requestTitle(req)}</span>
            <code>{req.requestId}</code>
          </div>
          <div class="request-actions">
            <button
              type="button"
              onclick={() => decide(req, 'accept')}
              disabled={pendingAction === req.requestId}
            >
              Accept
            </button>
            <button
              type="button"
              onclick={() => decide(req, 'decline')}
              disabled={pendingAction === req.requestId}
            >
              Decline
            </button>
          </div>
        </div>
      {/each}
    {/if}
  </section>

  <section class="panel graph">
    <header>
      <span>Work Graph</span>
      <strong>{threadGraph.length}</strong>
    </header>
    {#if threadGraph.length === 0}
      <p class="empty">No sub-agent activity yet.</p>
    {:else}
      {#each threadGraph as node (node.id)}
        <article class="agent-node" data-status={node.status ?? 'unknown'}>
          <div class="node-top">
            <span class="tool">{node.tool ?? 'agent'}</span>
            <span class="status">{node.status ?? 'unknown'}</span>
          </div>
          {#if node.prompt}
            <p>{node.prompt}</p>
          {/if}
          {#if node.receiverThreadIds.length > 0}
            <div class="receivers">
              {#each node.receiverThreadIds as id (id)}
                <code>{id}</code>
              {/each}
            </div>
          {/if}
          {#if Object.keys(node.agentsStates).length > 0}
            <div class="states">
              {#each Object.entries(node.agentsStates) as [id, state] (id)}
                <span><code>{id}</code>{stateLabel(state)}</span>
              {/each}
            </div>
          {/if}
        </article>
      {/each}
    {/if}
  </section>

  <section class="panel events">
    <header>
      <span>Event Stream</span>
      <strong>{threadEvents.length}</strong>
    </header>
    {#if threadEvents.length === 0}
      <p class="empty">No raw Codex events for this thread.</p>
    {:else}
      <ol>
        {#each threadEvents as ev (ev.id)}
          <li>
            <span class="event-method">{shortMethod(ev.method)}</span>
            <span class="event-detail">{eventDetail(ev)}</span>
          </li>
        {/each}
      </ol>
    {/if}
  </section>
</aside>

<style>
  .inspector {
    width: 300px;
    min-width: 260px;
    max-width: 340px;
    min-height: 0;
    background: color-mix(in srgb, var(--bg-pane) 94%, var(--bg-elev));
    display: flex;
    flex-direction: column;
    gap: 10px;
    padding: 10px;
    overflow: hidden;
    flex-shrink: 0;
  }
  .panel {
    min-height: 0;
    border: 1px solid var(--border);
    border-radius: 6px;
    background: var(--bg);
    overflow: auto;
    flex-shrink: 0;
  }
  .panel.events {
    flex: 1;
  }
  .panel.approvals,
  .panel.graph {
    max-height: 32%;
  }
  .panel.hot {
    border-color: var(--accent, var(--text-muted));
  }
  header {
    display: flex;
    justify-content: space-between;
    align-items: center;
    padding: 7px 9px;
    border-bottom: 1px solid var(--border);
    color: var(--text-muted);
    font-size: 11px;
    text-transform: uppercase;
    letter-spacing: 0.04em;
  }
  header strong {
    color: var(--text);
    font-variant-numeric: tabular-nums;
  }
  .empty,
  .error {
    margin: 0;
    padding: 10px;
    color: var(--text-dim);
    font-size: 12px;
    line-height: 1.35;
  }
  .error {
    color: var(--danger);
  }
  .request,
  .agent-node {
    padding: 9px;
    border-bottom: 1px solid var(--border);
  }
  .request:last-child,
  .agent-node:last-child {
    border-bottom: 0;
  }
  .request-main,
  .node-top {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
  }
  .method,
  .tool {
    color: var(--text);
    font-size: 12px;
    font-weight: 600;
  }
  code,
  .status,
  .event-detail {
    color: var(--text-dim);
    font-family: var(--font-mono);
    font-size: 10.5px;
  }
  .request-actions {
    display: flex;
    gap: 6px;
    margin-top: 8px;
  }
  .request-actions button {
    border: 1px solid var(--border);
    border-radius: 5px;
    background: var(--bg-elev);
    color: var(--text);
    font-size: 11px;
    padding: 3px 7px;
    cursor: pointer;
  }
  .request-actions button:hover {
    border-color: var(--border-hi);
    background: var(--bg-hover);
    color: var(--text);
  }
  .request-actions button:disabled {
    opacity: 0.55;
    cursor: default;
  }
  .agent-node p {
    margin: 5px 0 0;
    color: var(--text-dim);
    font-size: 12px;
    line-height: 1.35;
    display: -webkit-box;
    -webkit-line-clamp: 3;
    -webkit-box-orient: vertical;
    overflow: hidden;
  }
  .receivers,
  .states {
    display: flex;
    flex-wrap: wrap;
    gap: 4px;
    margin-top: 7px;
  }
  .states span {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    color: var(--text-dim);
    font-size: 10.5px;
  }
  ol {
    list-style: none;
    margin: 0;
    padding: 4px 0;
  }
  li {
    display: grid;
    grid-template-columns: minmax(0, 1fr) auto;
    gap: 8px;
    padding: 4px 9px;
    align-items: baseline;
  }
  .event-method {
    color: var(--text);
    font-size: 11.5px;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
</style>
