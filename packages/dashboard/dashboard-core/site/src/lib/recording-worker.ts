// The recording replay runs off the main thread.
//
// A saved recording is a full Loro oplog: replaying a past moment means checking
// the document out to the frontier at that timestamp. That checkout is O(the
// op-distance travelled), and a real session's oplog is large (a long-running
// exec pane appends its stdout op by op), so a single far checkout can take many
// seconds. Doing it on the UI thread froze the page on every scrub tick and
// every playback frame. This worker owns the recording's `LoroDoc` and does all
// the import and checkout work here, so the main thread only ever renders the
// pane snapshots this worker posts back.
//
// loro-crdt is imported dynamically (not a static top-level import) on purpose:
// the site is bundled to a single self-contained HTML with the CDN import marked
// external, and an inlined worker chunk turns a *static* external import into an
// undefined global. A dynamic `import()` survives that bundling as a real
// runtime import, matching how the main thread loads loro.
import type { LoroDoc, OpId } from 'https://esm.sh/loro-crdt@1';
import type { PaneRecord } from './types';

// One change reduced to what scrubbing needs: the id of its last op (the version
// to check out to land exactly after it) and its timestamp. Mirrors the shape the
// main thread used before this moved into the worker.
interface Mark {
  peer: string;
  counter: number;
  ts: number;
  lamport: number;
}

// Messages the main thread sends in.
export type RecordingRequest =
  | { type: 'load'; id: string; bytes: ArrayBuffer }
  | { type: 'seek'; ts: number };

// Messages this worker posts back. `id` echoes the load request so a reply for a
// superseded recording (the user switched fast) can be ignored.
export type RecordingResponse =
  | {
      type: 'loaded';
      id: string;
      minTs: number;
      maxTs: number;
      changeCount: number;
      startTs: number;
      panes: Record<string, PaneRecord>;
    }
  | { type: 'frame'; id: string; ts: number; panes: Record<string, PaneRecord> }
  | { type: 'error'; id: string; message: string };

let LoroDocCtor: typeof LoroDoc | null = null;
let doc: LoroDoc | null = null;
let marks: Mark[] = [];
let currentId = '';

// A pending seek target coalesces to the newest: a drag fires many `input`
// events, but each checkout is expensive, so we only ever compute the latest
// requested position and drop the ones the user scrubbed past.
let pendingTs: number | null = null;
let seeking = false;

async function loro(): Promise<typeof LoroDoc> {
  if (!LoroDocCtor) {
    const mod = await import('https://esm.sh/loro-crdt@1');
    LoroDocCtor = mod.LoroDoc;
  }
  return LoroDocCtor;
}

// Rebuild the change index from the oplog. The aggregator is the sole editor, so
// the marks sort cleanly by time then lamport and the last mark at or before a
// timestamp fully describes the state at that moment.
function rebuildIndex(d: LoroDoc): void {
  const next: Mark[] = [];
  for (const [peer, changes] of d.getAllChanges()) {
    for (const c of changes) {
      next.push({
        peer: String(peer),
        counter: c.counter + c.length - 1,
        ts: c.timestamp,
        lamport: c.lamport,
      });
    }
  }
  next.sort((a, b) => a.ts - b.ts || a.lamport - b.lamport);
  marks = next;
}

// The version (frontier) at or before `ts`: the last mark at or before it.
function frontierAt(ts: number): OpId[] {
  let lo = 0;
  let hi = marks.length - 1;
  let best = -1;
  while (lo <= hi) {
    const mid = (lo + hi) >> 1;
    if (marks[mid].ts <= ts) {
      best = mid;
      lo = mid + 1;
    } else {
      hi = mid - 1;
    }
  }
  if (best < 0) best = 0;
  const mark = marks[best];
  return mark ? [{ peer: mark.peer, counter: mark.counter }] : [];
}

function panesAt(d: LoroDoc, ts: number): Record<string, PaneRecord> {
  const frontier = frontierAt(ts);
  if (frontier.length) d.checkout(frontier);
  return (d.toJSON().panes ?? {}) as Record<string, PaneRecord>;
}

// Run the newest pending seek, then any seek that arrived while it ran, until the
// queue drains. Serialising this way means an expensive checkout never stacks up
// behind the many events a slider drag emits.
function drainSeek(): void {
  if (seeking || pendingTs === null || !doc) return;
  seeking = true;
  const d = doc;
  const id = currentId;
  // Let the message loop breathe between checkouts so a fresh, newer target can
  // land and supersede a stale one before we spend seconds on it.
  queueMicrotask(() => {
    // A `load` may have arrived between scheduling and running this microtask; it
    // sets `currentId` synchronously before its first await, so a changed id means
    // `d` is a stale document we must not check out or reply for.
    if (id !== currentId) {
      seeking = false;
      return;
    }
    const ts = pendingTs;
    pendingTs = null;
    if (ts === null) {
      seeking = false;
      return;
    }
    try {
      const panes = panesAt(d, ts);
      const reply: RecordingResponse = { type: 'frame', id, ts, panes };
      self.postMessage(reply);
    } catch (err) {
      const reply: RecordingResponse = {
        type: 'error',
        id,
        message: String(err),
      };
      self.postMessage(reply);
    }
    seeking = false;
    drainSeek();
  });
}

self.onmessage = async (event: MessageEvent<RecordingRequest>) => {
  const msg = event.data;
  if (msg.type === 'load') {
    currentId = msg.id;
    pendingTs = null;
    seeking = false;
    try {
      const Ctor = await loro();
      doc = new Ctor();
      doc.import(new Uint8Array(msg.bytes));
      rebuildIndex(doc);
      const minTs = marks.length ? marks[0].ts : 0;
      const maxTs = marks.length ? marks[marks.length - 1].ts : 0;
      // Open the recording parked at its start, like the old main-thread path.
      const panes = panesAt(doc, minTs);
      const reply: RecordingResponse = {
        type: 'loaded',
        id: msg.id,
        minTs,
        maxTs,
        changeCount: marks.length,
        startTs: minTs,
        panes,
      };
      self.postMessage(reply);
    } catch (err) {
      const reply: RecordingResponse = { type: 'error', id: msg.id, message: String(err) };
      self.postMessage(reply);
    }
    return;
  }
  // seek
  pendingTs = msg.ts;
  drainSeek();
};
