<script lang="ts">
  import { LoroDoc } from 'loro-crdt';
  import { onDestroy } from 'svelte';

  type AgentStatus = 'idle' | 'thinking' | 'editing' | 'reviewing' | 'blocked';

  type ParticipantRecord = {
    name: string;
    color: string;
    focus: string;
    draft: string;
    codexTask: string;
    codexStatus: AgentStatus;
    lastSeenMs: number;
  };

  type ParticipantView = {
    id: string;
    record: ParticipantRecord;
  };

  const colors = ['#2f80ed', '#15a46e', '#c05621', '#8b5cf6', '#d12b6a'];
  const statuses: AgentStatus[] = ['idle', 'thinking', 'editing', 'reviewing', 'blocked'];

  const selfId = (() => {
    const saved = localStorage.getItem('room-id');
    if (saved) {
      return saved;
    }
    const fresh = crypto.randomUUID();
    localStorage.setItem('room-id', fresh);
    return fresh;
  })();

  const doc = new LoroDoc();
  const participants = doc.getMap('participants');

  let connected = $state(false);
  let participantViews = $state<ParticipantView[]>([]);
  let name = $state(localStorage.getItem('room-name') ?? 'Teammate');
  let color = $state(colors[Math.abs(hash(selfId)) % colors.length]);
  let focus = $state('overview');
  let draft = $state('');
  let task = $state('reviewing the current branch');
  let status = $state<AgentStatus>('thinking');

  let socket: WebSocket | null = null;
  let reconnectTimer: number | null = null;
  let suppressMirror = false;
  let destroyed = false;

  const unsubscribeDoc = doc.subscribe(() => {
    refreshFromDoc();
  });

  const unsubscribeLocal = doc.subscribeLocalUpdates((bytes) => {
    if (socket && socket.readyState === WebSocket.OPEN) {
      socket.send(bytes);
    }
  });

  onDestroy(() => {
    destroyed = true;
    removeSelf();
    unsubscribeDoc();
    unsubscribeLocal();
    window.removeEventListener('pagehide', removeSelf);
    if (reconnectTimer !== null) {
      window.clearTimeout(reconnectTimer);
    }
    socket?.close();
  });

  connect();
  window.addEventListener('pagehide', removeSelf);

  $effect(() => {
    if (suppressMirror) {
      return;
    }
    void name;
    void color;
    void focus;
    void draft;
    void task;
    void status;
    publishSelf();
  });

  const selfRecord = $derived(participantViews.find((view) => view.id === selfId)?.record);
  const otherViews = $derived(participantViews.filter((view) => view.id !== selfId));

  function connect() {
    const protocol = window.location.protocol === 'https:' ? 'wss:' : 'ws:';
    const next = new WebSocket(`${protocol}//${window.location.host}/ws`);
    next.binaryType = 'arraybuffer';
    next.addEventListener('open', () => {
      connected = true;
      next.send(`room-id:${selfId}`);
      publishSelf(true);
    });
    next.addEventListener('close', () => {
      connected = false;
      if (socket === next) {
        socket = null;
      }
      if (destroyed || reconnectTimer !== null) {
        return;
      }
      reconnectTimer = window.setTimeout(() => {
        reconnectTimer = null;
        connect();
      }, 1000);
    });
    next.addEventListener('message', (event) => {
      if (!(event.data instanceof ArrayBuffer)) {
        return;
      }
      try {
        doc.import(new Uint8Array(event.data));
      } catch {
        // Skip frames the server sent before our state caught up.
      }
    });
    socket = next;
  }

  function publishSelf(force = false) {
    const previous = participants.get(selfId) as ParticipantRecord | undefined;
    const record: ParticipantRecord = {
      name,
      color,
      focus,
      draft,
      codexTask: task,
      codexStatus: status,
      lastSeenMs: Date.now()
    };
    if (!force && previous && shallowEqualExceptLastSeen(previous, record)) {
      return;
    }
    participants.set(selfId, record);
    doc.commit();
    localStorage.setItem('room-name', name);
  }

  function refreshFromDoc() {
    const snapshot = participants.toJSON() as Record<string, unknown>;
    const next: ParticipantView[] = [];
    for (const [id, value] of Object.entries(snapshot)) {
      const record = parseParticipantRecord(value);
      if (record) {
        next.push({ id, record });
      }
    }
    next.sort((a, b) => a.record.name.localeCompare(b.record.name));
    participantViews = next;

    const mine = parseParticipantRecord(snapshot[selfId]);
    if (mine) {
      suppressMirror = true;
      try {
        if (mine.name !== name) {
          name = mine.name;
        }
        if (mine.color !== color) {
          color = mine.color;
        }
        if (mine.focus !== focus) {
          focus = mine.focus;
        }
        if (mine.draft !== draft) {
          draft = mine.draft;
        }
        if (mine.codexTask !== task) {
          task = mine.codexTask;
        }
        if (mine.codexStatus !== status) {
          status = mine.codexStatus;
        }
      } finally {
        suppressMirror = false;
      }
    }
  }

  function shallowEqualExceptLastSeen(left: ParticipantRecord, right: ParticipantRecord) {
    return (
      left.name === right.name &&
      left.color === right.color &&
      left.focus === right.focus &&
      left.draft === right.draft &&
      left.codexTask === right.codexTask &&
      left.codexStatus === right.codexStatus
    );
  }

  function removeSelf() {
    participants.delete(selfId);
    doc.commit();
  }

  function parseParticipantRecord(value: unknown): ParticipantRecord | null {
    if (!value || typeof value !== 'object') {
      return null;
    }
    const record = value as Record<string, unknown>;
    if (
      typeof record.name !== 'string' ||
      typeof record.color !== 'string' ||
      typeof record.focus !== 'string' ||
      typeof record.draft !== 'string' ||
      typeof record.codexTask !== 'string' ||
      !isAgentStatus(record.codexStatus) ||
      typeof record.lastSeenMs !== 'number'
    ) {
      return null;
    }
    return {
      name: record.name,
      color: record.color,
      focus: record.focus,
      draft: record.draft,
      codexTask: record.codexTask,
      codexStatus: record.codexStatus,
      lastSeenMs: record.lastSeenMs
    };
  }

  function isAgentStatus(value: unknown): value is AgentStatus {
    return typeof value === 'string' && statuses.includes(value as AgentStatus);
  }

  function hash(value: string) {
    return [...value].reduce((accumulator, char) => (accumulator * 31 + char.charCodeAt(0)) | 0, 0);
  }
</script>

<main>
  <section class="toolbar">
    <div>
      <h1>Room</h1>
      <p>{connected ? 'live Loro session' : 'reconnecting'}</p>
    </div>
    <label>
      <span>Name</span>
      <input bind:value={name} />
    </label>
  </section>

  <section class="workspace">
    <aside>
      <div class="panel-title">Team</div>
      {#each participantViews as view (view.id)}
        <article class:self={view.id === selfId}>
          <div class="avatar" style={`background:${view.record.color}`}>
            {view.record.name.slice(0, 1)}
          </div>
          <div>
            <strong>{view.record.name}</strong>
            <span>{view.record.focus}</span>
          </div>
          <small>{view.record.codexStatus}</small>
        </article>
      {/each}
    </aside>

    <section class="stage">
      <div class="field-grid">
        <label>
          <span>Viewing</span>
          <input bind:value={focus} placeholder="packages/room/src/main.rs" />
        </label>
        <label>
          <span>Codex task</span>
          <input bind:value={task} />
        </label>
        <label>
          <span>Status</span>
          <select bind:value={status}>
            {#each statuses as item (item)}
              <option value={item}>{item}</option>
            {/each}
          </select>
        </label>
      </div>

      <label class="draft">
        <span>Typing</span>
        <textarea bind:value={draft} placeholder="Share the prompt or note you are composing."></textarea>
      </label>

      <div class="board">
        {#if selfRecord}
          <div class="lane active">
            <span class="eyebrow">You</span>
            <h2>{selfRecord.codexTask}</h2>
            <p>{selfRecord.draft || 'No draft text yet.'}</p>
          </div>
        {/if}
        {#each otherViews as view (view.id)}
          <div class="lane">
            <span class="eyebrow" style={`color:${view.record.color}`}>{view.record.name}</span>
            <h2>{view.record.codexTask}</h2>
            <p>{view.record.draft || `${view.record.name} is not typing right now.`}</p>
            <footer>{view.record.focus}</footer>
          </div>
        {/each}
      </div>
    </section>
  </section>
</main>
