// Reactive stores that mirror one or more room-server instances.

import { writable, derived, get, type Readable, type Writable } from 'svelte/store';
import * as api from './api';
import { roomServers, type RoomServer } from './backend';
import { makeDoc, type RoomDoc } from './loro';
import { loadIdentity } from './identity';
import type { Message, ServerEvent, Thread } from './types';
import { createRoomTransport, type RoomTransport } from './transport';
import { ServerEventSchema } from './wire';
import { startVoice, type VoiceController } from './audio';
import { drafts, draftAsThread } from './drafts';

type ConnectionState = 'connecting' | 'open' | 'closed';

export interface ServerThread extends Thread {
  server_id: string;
  server_name: string;
}

export const OPTIMISTIC_ID_PREFIX = 'local-';

export interface RoomStore {
  server: RoomServer;
  doc: RoomDoc;
  connection: Readable<ConnectionState>;
  threads: Readable<Map<string, Thread>>;
  threadsList: Readable<Thread[]>;
  messagesFor(threadId: string): Readable<Message[] | undefined>;
  ensureMessages(threadId: string): Promise<void>;
  appendOptimisticUserMessage(threadId: string, text: string, images?: string[]): string;
  dropOptimisticMessage(threadId: string, id: string): void;
  close(): void;
}

const stores = new Map<string, RoomStore>();

export function roomFor(serverId: string): RoomStore {
  const existing = stores.get(serverId);
  if (existing) return existing;
  const server = get(roomServers).find((s) => s.id === serverId);
  if (!server) throw new Error('unknown room server: ' + serverId);
  const created = makeStore(server);
  stores.set(serverId, created);
  return created;
}

export const room = roomFor;

export function activeRoomStores(): RoomStore[] {
  const enabled = new Set(get(roomServers).filter((s) => s.enabled).map((s) => s.id));
  return [...stores.values()].filter((store) => enabled.has(store.server.id));
}

let previousServers = get(roomServers);
roomServers.subscribe((next) => {
  const nextById = new Map(next.map((s) => [s.id, s]));
  for (const prev of previousServers) {
    const current = nextById.get(prev.id);
    if (
      !current ||
      !current.enabled ||
      current.httpBase !== prev.httpBase
    ) {
      const store = stores.get(prev.id);
      store?.close();
      stores.delete(prev.id);
    }
  }
  previousServers = next;
});

export const allServerThreads: Readable<ServerThread[]> = readableDynamicThreads();

export const mergedThreadsList: Readable<ServerThread[]> = derived(
  [allServerThreads, drafts],
  ([serverThreads, draftsM]) => {
    const serverKeys = new Set(serverThreads.map((t) => threadKey(t.server_id, t.id)));
    const enabledServers = get(roomServers).filter((s) => s.enabled);
    const servers = new Map(enabledServers.map((s) => [s.id, s]));
    const draftList = [...draftsM.values()]
      .filter((d) => servers.has(d.server_id) && !serverKeys.has(threadKey(d.server_id, d.id)))
      .map((d) => draftAsThread(d, servers.get(d.server_id)?.name ?? d.server_id))
      .sort((a, b) => b.updated_ms - a.updated_ms);
    const serverSorted = [...serverThreads].sort((a, b) => b.updated_ms - a.updated_ms);
    return [...draftList, ...serverSorted];
  }
);

function readableDynamicThreads(): Readable<ServerThread[]> {
  return {
    subscribe(run) {
      const threadLists = new Map<string, Thread[]>();
      const unsubThreads = new Map<string, () => void>();

      function publish() {
        const servers = get(roomServers).filter((s) => s.enabled);
        const enabled = new Set(servers.map((s) => s.id));
        const names = new Map(servers.map((s) => [s.id, s.name]));
        const rows: ServerThread[] = [];
        for (const [serverId, list] of threadLists) {
          if (!enabled.has(serverId)) continue;
          for (const t of list) {
            rows.push({
              ...t,
              server_id: serverId,
              server_name: names.get(serverId) ?? serverId
            });
          }
        }
        run(rows.sort((a, b) => b.updated_ms - a.updated_ms));
      }

      function syncServers(servers: RoomServer[]) {
        const enabled = servers.filter((s) => s.enabled);
        const wanted = new Set(enabled.map((s) => s.id));
        for (const server of enabled) {
          if (unsubThreads.has(server.id) && stores.has(server.id)) continue;
          if (unsubThreads.has(server.id)) {
            unsubThreads.get(server.id)?.();
            unsubThreads.delete(server.id);
            threadLists.delete(server.id);
          }
          const store = roomFor(server.id);
          const unsub = store.threadsList.subscribe((threads) => {
            threadLists.set(server.id, threads);
            publish();
          });
          unsubThreads.set(server.id, unsub);
        }
        for (const [serverId, unsub] of unsubThreads) {
          if (wanted.has(serverId)) continue;
          unsub();
          unsubThreads.delete(serverId);
          threadLists.delete(serverId);
        }
        publish();
      }

      const unsubServers = roomServers.subscribe(syncServers);
      return () => {
        unsubServers();
        for (const unsub of unsubThreads.values()) unsub();
      };
    }
  };
}

function makeStore(server: RoomServer): RoomStore {
  const doc = makeDoc();
  const threadsMap: Writable<Map<string, Thread>> = writable(new Map());
  const messagesMap: Writable<Map<string, Message[]>> = writable(new Map());
  const connection: Writable<ConnectionState> = writable('connecting');
  const pendingMessageFetch = new Set<string>();

  const threadsList = derived(threadsMap, (m) =>
    [...m.values()].sort((a, b) => b.updated_ms - a.updated_ms)
  );

  function upsertThread(t: Thread) {
    threadsMap.update((m) => {
      m.set(t.id, t);
      return new Map(m);
    });
  }

  function appendMessage(threadId: string, message: Message) {
    messagesMap.update((m) => {
      const arr = m.get(threadId) ?? [];
      if (arr.some((x) => x.id === message.id)) return m;
      const stripped =
        message.role === 'user' && message.kind === 'user_prompt'
          ? arr.filter(
              (x) =>
                !x.id.startsWith(OPTIMISTIC_ID_PREFIX) ||
                x.role !== 'user' ||
                x.text !== message.text
            )
          : arr;
      const next = [...stripped, message].sort((a, b) => a.ts_ms - b.ts_ms);
      m.set(threadId, next);
      return new Map(m);
    });
  }

  function appendOptimisticUserMessage(
    threadId: string,
    text: string,
    images?: string[]
  ): string {
    const id = OPTIMISTIC_ID_PREFIX + crypto.randomUUID();
    const msg: Message = {
      id,
      thread_id: threadId,
      ts_ms: Date.now(),
      role: 'user',
      kind: 'user_prompt',
      text,
      tool_name: null,
      tool_use_id: null,
      tool_input: null,
      result: null,
      patch: null,
      images: images && images.length > 0 ? images : undefined
    };
    messagesMap.update((m) => {
      const arr = m.get(threadId) ?? [];
      m.set(threadId, [...arr, msg]);
      return new Map(m);
    });
    return id;
  }

  function dropOptimisticMessage(threadId: string, id: string) {
    if (!id.startsWith(OPTIMISTIC_ID_PREFIX)) return;
    messagesMap.update((m) => {
      const arr = m.get(threadId);
      if (!arr) return m;
      const idx = arr.findIndex((x) => x.id === id);
      if (idx === -1) return m;
      m.set(threadId, [...arr.slice(0, idx), ...arr.slice(idx + 1)]);
      return new Map(m);
    });
  }

  function updateMessage(threadId: string, message: Message) {
    messagesMap.update((m) => {
      const arr = m.get(threadId);
      if (!arr) return m;
      const idx = arr.findIndex((x) => x.id === message.id);
      const next =
        idx === -1
          ? [...arr, message]
          : [...arr.slice(0, idx), message, ...arr.slice(idx + 1)];
      m.set(threadId, next.sort((a, b) => a.ts_ms - b.ts_ms));
      return new Map(m);
    });
  }

  function archiveThread(threadId: string) {
    threadsMap.update((m) => {
      const t = m.get(threadId);
      if (!t) return m;
      m.set(threadId, { ...t, status: 'archived' });
      return new Map(m);
    });
  }

  function applyEvent(ev: ServerEvent) {
    switch (ev.type) {
      case 'bootstrap':
        threadsMap.set(new Map(ev.threads.map((t) => [t.id, t])));
        break;
      case 'thread-upsert':
        upsertThread(ev.thread);
        break;
      case 'message-append':
        appendMessage(ev.thread_id, ev.message);
        break;
      case 'message-update':
        updateMessage(ev.thread_id, ev.message);
        break;
      case 'thread-archive':
        archiveThread(ev.thread_id);
        break;
      case 'ping':
        break;
    }
  }

  let transport: RoomTransport = server.managed ? createClosedTransport() : createRoomTransport(server.id);
  let voice: VoiceController | null = null;
  let detachLoroSubscriber: (() => void) | null = null;

  transport.onState((state) => {
    connection.set(state);
    if (state === 'open') {
      detachLoroSubscriber?.();
      const handleIncoming = doc.attachSocket((frame) => {
        transport.sendLoro(new Uint8Array(frame));
      });
      detachLoroSubscriber = transport.onLoroFrame((bytes) => handleIncoming(bytes));
      doc.setSelf(loadIdentity(), { online: true });
      if (!voice) {
        void startVoice(transport)
          .then((controller) => {
            voice = controller;
          })
          .catch((err) => {
            console.warn('room: voice loop not started', err);
          });
      }
    } else if (state === 'closed') {
      detachLoroSubscriber?.();
      detachLoroSubscriber = null;
      doc.detachSocket();
    }
  });
  // Validate every frame against the wire schema before trusting it. A
  // failure means a server/client version skew or a corrupt frame: drop
  // it and log the zod issues rather than let a malformed event throw
  // deep in a store update. On success the original frame is used as-is.
  transport.onJson((ev) => {
    const parsed = ServerEventSchema.safeParse(ev);
    if (parsed.success) {
      applyEvent(ev as ServerEvent);
    } else {
      console.warn('room: dropped malformed server event', parsed.error.issues, ev);
    }
  });

  async function refreshThreads() {
    try {
      const resp = await api.listThreads(server.id, { limit: 200 });
      threadsMap.set(new Map(resp.threads.map((t) => [t.id, t])));
    } catch (err) {
      console.warn('room: thread refresh failed', err);
    }
  }

  void refreshThreads();

  const heartbeat = setInterval(() => {
    if (get(connection) !== 'open') return;
    doc.setSelf(loadIdentity(), { online: true }, { heartbeat: true });
  }, 12000);

  const threadRefresh = setInterval(() => {
    void refreshThreads();
  }, server.managed ? 3000 : 10000);

  async function ensureMessages(threadId: string) {
    if (get(messagesMap).has(threadId)) return;
    if (pendingMessageFetch.has(threadId)) return;
    pendingMessageFetch.add(threadId);
    try {
      const resp = await api.listMessages(server.id, threadId, { limit: 500 });
      messagesMap.update((m) => {
        const existing = m.get(threadId) ?? [];
        m.set(threadId, mergeById([...resp.messages, ...existing]));
        return new Map(m);
      });
    } catch (err) {
      console.warn('room: ensureMessages failed', err);
    } finally {
      pendingMessageFetch.delete(threadId);
    }
  }

  function messagesFor(threadId: string): Readable<Message[] | undefined> {
    return derived(messagesMap, (m) => m.get(threadId));
  }

  function close() {
    clearInterval(heartbeat);
    clearInterval(threadRefresh);
    voice?.stop();
    detachLoroSubscriber?.();
    doc.detachSocket();
    transport.close();
  }

  return {
    server,
    doc,
    connection,
    threads: threadsMap,
    threadsList,
    messagesFor,
    ensureMessages,
    appendOptimisticUserMessage,
    dropOptimisticMessage,
    close
  };
}

function mergeById(arr: Message[]): Message[] {
  const seen = new Map<string, Message>();
  for (const m of arr) seen.set(m.id, m);
  return [...seen.values()].sort((a, b) => a.ts_ms - b.ts_ms);
}

function createClosedTransport(): RoomTransport {
  return {
    state: () => 'closed',
    onState: () => () => {},
    onJson: () => () => {},
    onLoroFrame: () => () => {},
    onDatagram: () => () => {},
    sendLoro: () => {},
    sendDatagram: () => {},
    close: () => {}
  };
}

function threadKey(serverId: string, threadId: string): string {
  return serverId + ':' + threadId;
}

if (typeof window !== 'undefined') {
  const sayGoodbye = () => {
    for (const store of stores.values()) {
      store.doc.setSelf(loadIdentity(), { online: false });
    }
  };
  window.addEventListener('pagehide', sayGoodbye);
  window.addEventListener('beforeunload', sayGoodbye);
}
