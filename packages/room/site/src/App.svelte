<script lang="ts">
  type AgentStatus = 'idle' | 'thinking' | 'editing' | 'reviewing' | 'blocked';

  type Participant = {
    id: string;
    name: string;
    color: string;
    focus: string;
    draft: string;
    codex: {
      task: string;
      status: AgentStatus;
    };
    lastSeenMs: number;
  };

  type SnapshotEvent = {
    type: 'snapshot';
    state: {
      participants: Record<string, Participant>;
    };
  };

  const colors = ['#2f80ed', '#15a46e', '#c05621', '#8b5cf6', '#d12b6a'];
  const statuses: AgentStatus[] = ['idle', 'thinking', 'editing', 'reviewing', 'blocked'];
  const savedId = localStorage.getItem('room-id');
  const selfId = savedId ?? crypto.randomUUID();
  localStorage.setItem('room-id', selfId);

  let socket: WebSocket | null = null;
  let connected = false;
  let name = localStorage.getItem('room-name') ?? 'Teammate';
  let color = colors[Math.abs(hash(selfId)) % colors.length];
  let focus = 'overview';
  let draft = '';
  let task = 'reviewing the current branch';
  let status: AgentStatus = 'thinking';
  let participants: Participant[] = [];

  $: self = participants.find((participant) => participant.id === selfId);
  $: others = participants.filter((participant) => participant.id !== selfId);

  connect();

  function connect() {
    const protocol = window.location.protocol === 'https:' ? 'wss:' : 'ws:';
    socket = new WebSocket(`${protocol}//${window.location.host}/ws`);
    socket.addEventListener('open', () => {
      connected = true;
      sendPresence();
      sendCodex();
      sendFocus();
      sendDraft();
    });
    socket.addEventListener('close', () => {
      connected = false;
      window.setTimeout(connect, 1000);
    });
    socket.addEventListener('message', (message) => {
      const event = JSON.parse(message.data) as SnapshotEvent;
      if (event.type === 'snapshot') {
        participants = Object.values(event.state.participants).sort((left, right) =>
          left.name.localeCompare(right.name)
        );
      }
    });
    window.addEventListener('beforeunload', () => send({ type: 'leave', id: selfId }));
  }

  function send(event: object) {
    if (socket?.readyState === WebSocket.OPEN) {
      socket.send(JSON.stringify(event));
    }
  }

  function sendPresence() {
    localStorage.setItem('room-name', name);
    send({ type: 'presence', id: selfId, name, color });
  }

  function sendFocus() {
    send({ type: 'focus', id: selfId, focus });
  }

  function sendDraft() {
    send({ type: 'draft', id: selfId, draft });
  }

  function sendCodex() {
    send({ type: 'codex', id: selfId, task, status });
  }

  function hash(value: string) {
    return [...value].reduce((accumulator, char) => (accumulator * 31 + char.charCodeAt(0)) | 0, 0);
  }
</script>

<main>
  <section class="toolbar">
    <div>
      <h1>Room</h1>
      <p>{connected ? 'live multiplayer session' : 'reconnecting'}</p>
    </div>
    <label>
      <span>Name</span>
      <input bind:value={name} on:input={sendPresence} />
    </label>
  </section>

  <section class="workspace">
    <aside>
      <div class="panel-title">Team</div>
      {#each participants as participant}
        <article class:self={participant.id === selfId}>
          <div class="avatar" style={`background:${participant.color}`}>{participant.name.slice(0, 1)}</div>
          <div>
            <strong>{participant.name}</strong>
            <span>{participant.focus}</span>
          </div>
          <small>{participant.codex.status}</small>
        </article>
      {/each}
    </aside>

    <section class="stage">
      <div class="field-grid">
        <label>
          <span>Viewing</span>
          <input bind:value={focus} on:input={sendFocus} placeholder="packages/room/src/main.rs" />
        </label>
        <label>
          <span>Codex task</span>
          <input bind:value={task} on:input={sendCodex} />
        </label>
        <label>
          <span>Status</span>
          <select bind:value={status} on:change={sendCodex}>
            {#each statuses as item}
              <option value={item}>{item}</option>
            {/each}
          </select>
        </label>
      </div>

      <label class="draft">
        <span>Typing</span>
        <textarea bind:value={draft} on:input={sendDraft} placeholder="Share the prompt or note you are composing."></textarea>
      </label>

      <div class="board">
        {#if self}
          <div class="lane active">
            <span class="eyebrow">You</span>
            <h2>{self.codex.task}</h2>
            <p>{self.draft || 'No draft text yet.'}</p>
          </div>
        {/if}
        {#each others as participant}
          <div class="lane">
            <span class="eyebrow" style={`color:${participant.color}`}>{participant.name}</span>
            <h2>{participant.codex.task}</h2>
            <p>{participant.draft || `${participant.name} is not typing right now.`}</p>
            <footer>{participant.focus}</footer>
          </div>
        {/each}
      </div>
    </section>
  </section>
</main>
