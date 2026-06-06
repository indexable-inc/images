// Loro CRDT wrapper for the shared room.
//
// One LoroDoc per browser. Two flavors of top-level state:
//
//   - `presence` (LoroMap): peer id → JSON-encoded `PresenceRecord`.
//     Each peer only writes its own key, so this is last-write-wins
//     per peer with no contention. Carries ephemeral state — who's
//     online, who's viewing what, where their scroll/cursor sits.
//
//   - composer body per thread: a root LoroText at the name
//     `composer:<threadId>`. Root containers have a deterministic id
//     (`cid:root-<name>:Text`), so two peers calling
//     `doc.getText("composer:X")` get the same logical container and
//     their edits merge through the text CRDT. Storing each composer
//     under a parent LoroMap looks tidier but loses this: every peer
//     that calls `map.getOrCreateContainer(key, new LoroText())`
//     mints a fresh ContainerID and Loro resolves the resulting map
//     conflict with last-write-wins, silently dropping one peer's
//     content the first time both windows open a thread for which no
//     container exists yet.
//
// Updates ride the room WebSocket as binary frames. Loro is the
// source of truth for small shared state (presence, composer drafts
// in flight). Persisted thread/message data still lives in the
// server's SQLite store and flows in through the existing event
// stream.

import { LoroDoc } from 'loro-crdt';
import { readable, writable, type Readable, type Writable } from 'svelte/store';
import type { Identity } from './identity';

export interface PresenceEntry {
  id: string;
  name: string;
  /** GitHub username when the peer set their identity to `kind:
   *  'github'`. Null for anonymous peers; consumers should fall
   *  back to the deterministic identicon in that case. */
  github: string | null;
  online: boolean;
  viewing_thread_id: string | null;
  typing_thread_id: string | null;
  /** Caret offset (UTF-16 code units) where the peer's cursor sits
   *  inside the composer LoroText for `typing_thread_id`. Null when
   *  unknown. Integer offsets can drift slightly under concurrent
   *  edits — a tradeoff against carrying an encoded Loro `Cursor`
   *  through the JSON presence payload. For a short chat composer
   *  the drift window is sub-frame. */
  typing_cursor: number | null;
  /** 0..1 fraction down the transcript the user has scrolled to,
   *  or null when they're not in a thread / haven't scrolled yet. */
  scroll_pct: number | null;
  /** 0..1 fraction of the transcript that's currently visible in
   *  the user's viewport. Combined with scroll_pct this lets
   *  remote peers see exactly what range each viewer can see. */
  viewport_pct: number | null;
  /** Last time the peer's WebSocket was alive — bumped by the 12s
   *  heartbeat *and* by every real interaction. Use this for "is
   *  the connection still healthy" decisions. */
  last_seen_ms: number;
  /** Last time the peer actually interacted (typed, moved the
   *  mouse, scrolled, navigated). Heartbeats DO NOT bump this, so
   *  a peer whose tab is idle keeps `last_seen_ms` fresh while
   *  `last_active_ms` ages — the gap is what drives the zzz idle
   *  indicator. */
  last_active_ms: number;
}

interface PresenceRecord {
  name: string;
  github?: string | null;
  online: boolean;
  viewing_thread_id: string | null;
  typing_thread_id: string | null;
  typing_cursor: number | null;
  scroll_pct: number | null;
  viewport_pct: number | null;
  last_seen_ms: number;
  last_active_ms: number;
}

export interface SetSelfOptions {
  /** When true, treat this update as a liveness ping only: bump
   *  `last_seen_ms` but leave `last_active_ms` untouched. */
  heartbeat?: boolean;
}

/** Per-thread composer body, backed by a `LoroText`. Both peers
 *  viewing the same thread attach to the same container, so local
 *  writes via `update()` and incoming peer ops merge through the
 *  text CRDT instead of overwriting each other. */
export interface ComposerText {
  /** Readable store of the current text. Fires for both local
   *  writes (via `update`) and incoming peer ops. */
  text: Readable<string>;
  /** Snapshot the current value without subscribing. */
  current(): string;
  /** Diff `next` against the current text and apply the minimal
   *  insert/delete operations through the LoroText CRDT, then
   *  commit + flush. No-op when `next` equals current. */
  update(next: string): void;
}

/** One image attachment staged on the composer. `id` is peer-minted
 *  so any peer can remove the entry without coordinating indexes
 *  with the inserting peer. `dataUrl` is the same `data:image/*;
 *  base64,...` payload the message gets sent with. */
export interface ComposerAttachment {
  id: string;
  name: string;
  dataUrl: string;
}

/** Per-thread local list of composer image attachments. Image data
 *  URLs are large and must not enter the Loro update log, which the
 *  server persists and replays in full. */
export interface ComposerImages {
  list: Readable<ComposerAttachment[]>;
  current(): ComposerAttachment[];
  add(items: ComposerAttachment[]): void;
  remove(id: string): void;
  clear(): void;
}

export interface CodexEventRecord {
  id: string;
  tsMs: number;
  method: string;
  threadId: string | null;
  turnId: string | null;
  itemId: string | null;
  params: unknown;
}

export interface CodexRequestRecord {
  requestId: string;
  method: string | null;
  status: 'pending' | 'resolved' | string;
  tsMs: number;
  threadId?: string | null;
  params?: unknown;
}

export interface CodexWorkGraphNode {
  id: string;
  tool: string | null;
  status: string | null;
  senderThreadId: string | null;
  receiverThreadIds: string[];
  prompt: string | null;
  model: string | null;
  reasoningEffort: string | null;
  agentsStates: Record<string, unknown>;
}

/** One reviewer note attached to an agent-side message. Author is
 *  the peer identity at the time the note was written; the same
 *  peer can leave multiple notes on the same message. The whole
 *  point of this surface is to capture failure-mode patterns we
 *  later mine to update AGENTS.md, so the text is intentionally
 *  free-form. */
export interface Annotation {
  id: string;
  /** Peer id (`Identity.id`) that wrote the note. */
  author_id: string;
  /** Display name at write time; we snapshot it so a later rename
   *  does not silently rewrite history. */
  author_name: string;
  ts_ms: number;
  text: string;
}

export interface MessageAnnotations {
  list: Readable<Annotation[]>;
  current(): Annotation[];
  /** Append a new note. Generates the id and timestamp. */
  add(self: Identity, text: string): void;
  /** Remove a note by id. Any peer can remove any note — this is a
   *  small shared room, not a permissioned system. */
  remove(id: string): void;
}

export interface RoomDoc {
  presenceList: Readable<PresenceEntry[]>;
  codexEvents: Readable<CodexEventRecord[]>;
  codexPendingRequests: Readable<CodexRequestRecord[]>;
  codexWorkGraph: Readable<CodexWorkGraphNode[]>;
  setSelf(self: Identity, patch: Partial<PresenceRecord>, opts?: SetSelfOptions): void;
  composerText(threadId: string): ComposerText;
  composerImages(threadId: string): ComposerImages;
  /** Reviewer notes for one message id. Backed by a root LoroMap
   *  named `annotations:<messageId>`, key = annotation id, value =
   *  JSON-encoded `Annotation`. Concurrent adds from different
   *  peers each pick fresh annotation ids so they cannot collide. */
  annotationsFor(messageId: string): MessageAnnotations;
  attachSocket(send: (frame: ArrayBuffer) => void): (frame: Uint8Array) => void;
  detachSocket(): void;
}

/** Cache per-thread (or per-key) handles so repeat calls share one
 *  underlying container, one subscription, and one Svelte store.
 *  Used for both ComposerText and local ComposerImages — the entry
 *  shape differs, the cache shape doesn't. */
function perKey<T>(make: (id: string) => T): (id: string) => T {
  const cache = new Map<string, T>();
  return (id) => {
    const existing = cache.get(id);
    if (existing) return existing;
    const created = make(id);
    cache.set(id, created);
    return created;
  };
}

/** Wrap a Loro container as a Svelte readable. `snapshot` runs on
 *  every change to materialize the JS-side shape. The store fires
 *  the listener synchronously on attach (see the long comment in
 *  Composer.svelte) — callers must assign unconditionally, not gate
 *  on stale local state. */
function liveStore<T>(
  snapshot: () => T,
  subscribe: (notify: () => void) => () => void
): Readable<T> {
  return readable(snapshot(), (set) => subscribe(() => set(snapshot())));
}

export function makeDoc(): RoomDoc {
  const doc = new LoroDoc();
  doc.setPeerId(BigInt(`0x${randomPeerHex()}`));
  const presence = doc.getMap('presence');
  const codexEventsMap = doc.getMap('codex:events');
  const codexRequestsMap = doc.getMap('codex:requests');
  const codexWorkGraphMap = doc.getMap('codex:workGraph');

  const presenceList: Writable<PresenceEntry[]> = writable([]);

  const codexEvents = liveStore(snapshotCodexEvents, (notify) =>
    codexEventsMap.subscribe(notify)
  );
  const codexPendingRequests = liveStore(snapshotCodexRequests, (notify) =>
    codexRequestsMap.subscribe(notify)
  );
  const codexWorkGraph = liveStore(snapshotCodexWorkGraph, (notify) =>
    codexWorkGraphMap.subscribe(notify)
  );

  function parseJsonRecord<T>(encoded: unknown): T | null {
    if (typeof encoded !== 'string') return null;
    try {
      return JSON.parse(encoded) as T;
    } catch {
      return null;
    }
  }

  function snapshotCodexEvents(): CodexEventRecord[] {
    const out: CodexEventRecord[] = [];
    for (const [, encoded] of codexEventsMap.entries()) {
      const rec = parseJsonRecord<Partial<CodexEventRecord>>(encoded);
      if (!rec || typeof rec.id !== 'string' || typeof rec.method !== 'string') continue;
      out.push({
        id: rec.id,
        tsMs: typeof rec.tsMs === 'number' ? rec.tsMs : 0,
        method: rec.method,
        threadId: typeof rec.threadId === 'string' ? rec.threadId : null,
        turnId: typeof rec.turnId === 'string' ? rec.turnId : null,
        itemId: typeof rec.itemId === 'string' ? rec.itemId : null,
        params: rec.params
      });
    }
    out.sort((a, b) => a.tsMs - b.tsMs || a.id.localeCompare(b.id));
    return out.slice(-500);
  }

  function snapshotCodexRequests(): CodexRequestRecord[] {
    const out: CodexRequestRecord[] = [];
    for (const [id, encoded] of codexRequestsMap.entries()) {
      const rec = parseJsonRecord<Partial<CodexRequestRecord>>(encoded);
      if (!rec) continue;
      const requestId = typeof rec.requestId === 'string' ? rec.requestId : id;
      const status = typeof rec.status === 'string' ? rec.status : 'pending';
      if (status !== 'pending') continue;
      out.push({
        requestId,
        method: typeof rec.method === 'string' ? rec.method : null,
        status,
        tsMs: typeof rec.tsMs === 'number' ? rec.tsMs : 0,
        threadId: typeof rec.threadId === 'string' ? rec.threadId : null,
        params: rec.params
      });
    }
    out.sort((a, b) => a.tsMs - b.tsMs || a.requestId.localeCompare(b.requestId));
    return out;
  }

  function snapshotCodexWorkGraph(): CodexWorkGraphNode[] {
    const out: CodexWorkGraphNode[] = [];
    for (const [id, encoded] of codexWorkGraphMap.entries()) {
      const rec = parseJsonRecord<Record<string, unknown>>(encoded);
      if (!rec) continue;
      const receiver = rec.receiverThreadIds;
      out.push({
        id,
        tool: typeof rec.tool === 'string' ? rec.tool : null,
        status: typeof rec.status === 'string' ? rec.status : null,
        senderThreadId: typeof rec.senderThreadId === 'string' ? rec.senderThreadId : null,
        receiverThreadIds: Array.isArray(receiver)
          ? receiver.filter((v): v is string => typeof v === 'string')
          : [],
        prompt: typeof rec.prompt === 'string' ? rec.prompt : null,
        model: typeof rec.model === 'string' ? rec.model : null,
        reasoningEffort: typeof rec.reasoningEffort === 'string' ? rec.reasoningEffort : null,
        agentsStates:
          rec.agentsStates && typeof rec.agentsStates === 'object' && !Array.isArray(rec.agentsStates)
            ? (rec.agentsStates as Record<string, unknown>)
            : {}
      });
    }
    out.sort((a, b) => a.id.localeCompare(b.id));
    return out;
  }

  function snapshotPresence() {
    const out: PresenceEntry[] = [];
    const value = presence.toJSON() as Record<string, string> | undefined;
    if (value) {
      for (const [id, encoded] of Object.entries(value)) {
        if (typeof encoded !== 'string') continue;
        try {
          const rec = JSON.parse(encoded) as PresenceRecord;
          const last_seen_ms =
            typeof rec.last_seen_ms === 'number' ? rec.last_seen_ms : 0;
          out.push({
            id,
            name: rec.name ?? id,
            github:
              typeof rec.github === 'string' && rec.github.length > 0
                ? rec.github
                : null,
            online: rec.online === true,
            viewing_thread_id: rec.viewing_thread_id ?? null,
            typing_thread_id: rec.typing_thread_id ?? null,
            typing_cursor:
              typeof rec.typing_cursor === 'number' && rec.typing_cursor >= 0
                ? rec.typing_cursor
                : null,
            scroll_pct:
              typeof rec.scroll_pct === 'number' && rec.scroll_pct >= 0 && rec.scroll_pct <= 1
                ? rec.scroll_pct
                : null,
            viewport_pct:
              typeof rec.viewport_pct === 'number' &&
              rec.viewport_pct > 0 &&
              rec.viewport_pct <= 1
                ? rec.viewport_pct
                : null,
            last_seen_ms,
            // Older peers (pre-idle-indicator) don't ship
            // last_active_ms — fall back to last_seen_ms so they
            // still look "active" while connected instead of being
            // permanently flagged as idle.
            last_active_ms:
              typeof rec.last_active_ms === 'number' ? rec.last_active_ms : last_seen_ms
          });
        } catch {
          // skip malformed
        }
      }
    }
    out.sort((a, b) => b.last_seen_ms - a.last_seen_ms);
    presenceList.set(out);
  }

  let sendFrame: ((frame: ArrayBuffer) => void) | null = null;
  let lastSentVv = doc.version();

  // Track which peer ids we've already seen so we can detect when a
  // *new* peer appears in the doc — that's our cue to push our own
  // snapshot back out so the newcomer learns about us. Without this,
  // a peer that joins later would only learn about existing peers on
  // the next heartbeat (12s+ stale).
  const seenPeerIds = new Set<string>();
  let firstSnapshot = true;
  let respondTimer: ReturnType<typeof setTimeout> | null = null;

  /** Wrap raw bytes from `doc.export(...)` and ship them through the
   *  socket. No-op when the socket is down or the export was empty
   *  (no new ops since the last flush). Advances `lastSentVv` on
   *  success so the next `mode: 'update'` export starts from where
   *  this one left off. */
  function emit(bytes: Uint8Array) {
    if (!sendFrame || bytes.byteLength === 0) return;
    const buf = new ArrayBuffer(bytes.byteLength);
    new Uint8Array(buf).set(bytes);
    sendFrame(buf);
    lastSentVv = doc.version();
  }

  function pushSnapshot() {
    emit(doc.export({ mode: 'snapshot' }));
  }

  function scheduleSnapshotPush() {
    if (respondTimer) return;
    respondTimer = setTimeout(() => {
      respondTimer = null;
      pushSnapshot();
    }, 150);
  }

  function detectNewPeers() {
    let foundNew = false;
    const value = presence.toJSON() as Record<string, string> | undefined;
    if (value) {
      for (const id of Object.keys(value)) {
        if (!seenPeerIds.has(id)) {
          seenPeerIds.add(id);
          if (!firstSnapshot) foundNew = true;
        }
      }
    }
    firstSnapshot = false;
    if (foundNew) scheduleSnapshotPush();
  }

  doc.subscribe(() => {
    snapshotPresence();
    detectNewPeers();
  });

  function flushOutgoing() {
    emit(doc.export({ mode: 'update', from: lastSentVv }));
  }

  /** Seal the current pending ops and push the delta to peers in one
   *  step. Every Loro write path (`setSelf`, `composerText.update`)
   *  ends with this — no caller should be touching `doc.commit()` or
   *  `flushOutgoing()` directly. */
  function commitFlush() {
    doc.commit();
    flushOutgoing();
  }

  function setSelf(
    self: Identity,
    patch: Partial<PresenceRecord>,
    opts: SetSelfOptions = {}
  ) {
    const now = Date.now();
    const current = readSelf(self.id) ?? {
      name: self.name,
      github: null,
      online: true,
      viewing_thread_id: null,
      typing_thread_id: null,
      typing_cursor: null,
      scroll_pct: null,
      viewport_pct: null,
      last_seen_ms: now,
      last_active_ms: now
    };
    // Always derive github from the local identity — every setSelf
    // call refreshes it so flipping anon ⇄ github propagates to
    // peers without needing a dedicated rename hop.
    const githubFromSelf =
      self.kind === 'github' && self.github ? self.github : null;
    const next: PresenceRecord = {
      ...current,
      ...patch,
      name: patch.name ?? self.name,
      github: patch.github !== undefined ? patch.github : githubFromSelf,
      last_seen_ms: now,
      // Heartbeats only ping liveness; real updates also count as
      // activity so the idle indicator clears.
      last_active_ms: opts.heartbeat ? current.last_active_ms ?? now : now
    };
    presence.set(self.id, JSON.stringify(next));
    commitFlush();
  }

  function readSelf(id: string): PresenceRecord | null {
    const encoded = presence.get(id);
    if (typeof encoded !== 'string') return null;
    try {
      return JSON.parse(encoded) as PresenceRecord;
    } catch {
      return null;
    }
  }

  // Per-thread composer body. Root container id is
  // `cid:root-composer:<threadId>:Text`, identical on every peer, so
  // there's no racing on fresh containers and no last-write-wins on a
  // parent map slot.
  const composerText = perKey<ComposerText>((threadId) => {
    const text = doc.getText(`composer:${threadId}`);
    const current = () => text.toString();
    return {
      text: liveStore(current, (notify) => text.subscribe(notify)),
      current,
      update(next: string) {
        if (current() === next) return;
        // `update` diffs the new string against the current one
        // and emits the minimal insert/delete ops — the proper
        // CRDT shape, equivalent to what a textarea-aware diff
        // would compute by hand.
        text.update(next);
        commitFlush();
      }
    };
  });

  // Per-thread image attachment staging. This is intentionally local
  // browser state rather than a Loro container: a few screenshots can
  // be tens of MiB as data URLs, and every Loro frame is persisted by
  // the server's append-only update log until compaction exists.
  const composerImages = perKey<ComposerImages>(() => {
    let current: ComposerAttachment[] = [];
    const list = writable<ComposerAttachment[]>(current);
    const publish = (next: ComposerAttachment[]) => {
      current = next;
      list.set(current);
    };
    return {
      list,
      current: () => current,
      add(items: ComposerAttachment[]) {
        if (items.length === 0) return;
        publish([...current, ...items]);
      },
      remove(id: string) {
        const next = current.filter((a) => a.id !== id);
        if (next.length === current.length) return;
        publish(next);
      },
      clear() {
        if (current.length === 0) return;
        publish([]);
      }
    };
  });

  // Per-message reviewer notes. Like composers, each annotation set
  // is its own root container (`annotations:<messageId>`) so the
  // container id is deterministic across peers and concurrent first-
  // writers don't lose each other's ops to a parent-map LWW.
  const annotationsFor = perKey<MessageAnnotations>((messageId) => {
    const map = doc.getMap(`annotations:${messageId}`);
    const current = (): Annotation[] => {
      const out: Annotation[] = [];
      for (const [id, encoded] of map.entries()) {
        if (typeof encoded !== 'string') continue;
        try {
          const rec = JSON.parse(encoded) as Partial<Annotation>;
          if (
            typeof rec.author_id === 'string' &&
            typeof rec.author_name === 'string' &&
            typeof rec.ts_ms === 'number' &&
            typeof rec.text === 'string'
          ) {
            out.push({
              id,
              author_id: rec.author_id,
              author_name: rec.author_name,
              ts_ms: rec.ts_ms,
              text: rec.text
            });
          }
        } catch {
          // skip malformed
        }
      }
      out.sort((a, b) => a.ts_ms - b.ts_ms);
      return out;
    };
    return {
      list: liveStore(current, (notify) => map.subscribe(notify)),
      current,
      add(self, text) {
        const trimmed = text.trim();
        if (trimmed.length === 0) return;
        const id = newAnnotationId();
        const record: Annotation = {
          id,
          author_id: self.id,
          author_name: self.name,
          ts_ms: Date.now(),
          text: trimmed
        };
        map.set(id, JSON.stringify(record));
        commitFlush();
      },
      remove(id) {
        if (map.get(id) === undefined) return;
        map.delete(id);
        commitFlush();
      }
    };
  });

  function attachSocket(send: (frame: ArrayBuffer) => void) {
    sendFrame = send;
    // Send a snapshot on connect so the server (and other peers) see
    // this client's accumulated presence even after a reconnect.
    pushSnapshot();
    return (incoming: Uint8Array) => {
      try {
        doc.import(incoming);
        lastSentVv = doc.version();
      } catch (err) {
        console.warn('room: failed to import loro frame', err);
      }
    };
  }

  function detachSocket() {
    sendFrame = null;
  }

  return {
    presenceList,
    codexEvents,
    codexPendingRequests,
    codexWorkGraph,
    setSelf,
    composerText,
    composerImages,
    annotationsFor,
    attachSocket,
    detachSocket
  };
}

function newAnnotationId(): string {
  const arr = new Uint8Array(8);
  crypto.getRandomValues(arr);
  return [...arr].map((b) => b.toString(16).padStart(2, '0')).join('');
}

function randomPeerHex(): string {
  const arr = new Uint8Array(8);
  crypto.getRandomValues(arr);
  return [...arr].map((b) => b.toString(16).padStart(2, '0')).join('');
}

export const roomDoc = makeDoc();
