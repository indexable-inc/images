// Live pane state, the replay timeline, and the browser's own edits.
//
// The document is a CRDT with more than one writer. Producers own the `panes`
// root and stream it in through `/events`; a viewer owns the `inputs` root and
// posts its ops back out through `/apply`. A click on an input control IS a CRDT
// edit -- there is no event bus and no payload schema, so the same click twice is
// the same document, and the answer merges to every other viewer by the same
// path a producer's tick does.
//
// The hub records a millisecond timestamp on every change, so the imported
// document is a full recording: we keep the whole oplog, list its changes, and
// check the document out to any past version to replay it. `store` is the
// rendered pane set at the current view; `timeline` is the scrubber state.
//
// Two rendering paths share one `store`. The LIVE stream imports small
// incremental frames on the main thread and always shows the latest version —
// cheap, so it stays here. A saved RECORDING is replayed in a Web Worker
// (`recording-worker.ts`): checking a large oplog out to an arbitrary past
// version is O(the op-distance travelled) and takes seconds, which froze the UI
// when done per scrub tick on the main thread. The worker owns the recording's
// document and posts back the pane snapshot at each requested moment, so the main
// thread only renders.
import { LoroDoc, LoroText } from 'https://esm.sh/loro-crdt@1';
import type { DocJson, JsonValue } from 'https://esm.sh/loro-crdt@1';
import { paneScope, SCOPE_SEP } from './scope.ts';
import type { PaneRecord } from './types';
import { changeId, frontierAt, markOf, sortMarks, type Mark } from './frontier.ts';
import { summarize, type EditRow } from './edits.ts';
import { readPeers, type PeerInfo } from './peers.ts';
import RecordingWorker from './recording-worker.ts?worker&inline';
import type { RecordingRequest, RecordingResponse } from './recording-worker.ts';

export { SCOPE_SEP };
export type { EditRow };

export const store = $state({
  panes: {} as Record<string, PaneRecord>,
  // The `inputs` root: viewer-written answers, keyed by input id. LWW per key,
  // which is exactly right for "what did we decide".
  inputs: {} as Record<string, unknown>,
  // The `__peers` root: who each peer id is. Written by whoever knows -- the
  // aggregator for its agents, this browser for itself -- and always incomplete,
  // so every reader degrades (see peers.ts).
  peers: {} as Record<string, PeerInfo>,
  producers: 0,
  live: false,
  status: 'connecting',
});

// The edit history: every change in the oplog, newest last, with what it did.
// `marked` is the row the user clicked, which the document draws a marker at.
export const edits = $state({
  rows: [] as EditRow[],
  marked: null as string | null,
  localPeer: '',
});

// The outbound half of the write path. `pending` counts posts in flight so the
// chrome can say an edit is still travelling; `error` is a write the server
// refused, which must be visible rather than dropped -- a human who clicked
// "approve" and saw nothing happen has no way to know it did not land.
export const writes = $state({
  pending: 0,
  error: null as string | null,
});

export interface RecordingInfo {
  id: string;
  started_ms: number;
  updated_ms: number;
  bytes: number;
}

// The replay timeline. `source` is `'live'` for the SSE stream or a recording id
// for a loaded snapshot. `following` pins the view to the latest version (so a
// live frame advances it); scrubbing or playing detaches it at `position`.
// `seeking` is true while the worker is computing a replay frame, so the UI can
// show that a scrub is in flight rather than looking frozen.
export const timeline = $state({
  source: 'live' as string,
  following: true,
  playing: false,
  speed: 1,
  minTs: 0,
  maxTs: 0,
  position: 0,
  changeCount: 0,
  seeking: false,
  recordings: [] as RecordingInfo[],
});

// How many of the newest changes the history panel keeps. A long session's oplog
// runs to tens of thousands of changes and nobody scrolls that far; capping the
// window bounds both the DOM and the per-change op reads that build it.
const EDIT_WINDOW = 250;

let doc = newDoc();
// The live doc's change index, rebuilt on each frame. Only the live path uses it;
// a recording's index lives in the worker with its document.
let liveMarks: Mark[] = [];
// What each change did, memoised by change id: reading a change's ops crosses the
// WASM boundary, and a change is immutable once committed, so it is read once.
let editCache = new Map<string, EditRow>();
let es: EventSource | null = null;
let raf = 0;
let lastTick = 0;
// A `#t=` deep link to apply once the live history covers it.
let pendingSeek: number | null = null;
// Unsubscribes the local-update listener that feeds the write path.
let unsubscribeLocal: (() => void) | null = null;
// Whether this browser has announced itself in `__peers` yet. Registration rides
// along with the first real edit rather than happening on load, so a viewer that
// only reads adds nothing to anyone's history.
let registered = false;

// A document configured the way this app needs it.
//
// `setChangeMergeInterval(0)` stops loro folding consecutive local commits into
// one change, so one answer is one row in the edit history instead of an
// ever-growing blob. Detached editing is deliberately left OFF (the default): a
// write made while scrubbing a past version would fork the history, so `setInput`
// refuses instead.
function newDoc(): LoroDoc {
  const next = new LoroDoc();
  next.setChangeMergeInterval(0);
  return next;
}

// The replay worker and the recording it currently holds. Created lazily on the
// first recording load and reused across recordings.
let worker: Worker | null = null;
// The id whose frames we are currently rendering. A frame for any other id is a
// stale reply (the user switched recordings) and is dropped.
let activeRecordingId = '';
// A `#t=` position to scrub to once the recording finishes loading (a deep link
// opens the recording and then jumps to the shared moment).
let recordingSeekOnLoad: number | null = null;

function b64ToBytes(b64: string): Uint8Array {
  const bin = atob(b64);
  const out = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
  return out;
}

// Read one rendering of the document into `store`, deriving the producer count
// and status line. The live path and the worker's replay frames both funnel
// through here so they render identically -- including `inputs`, so a scrubbed
// moment shows the answers as they stood then rather than as they stand now.
function applyDoc(json: DocJson): void {
  const panes = (json.panes ?? {}) as Record<string, PaneRecord>;
  store.panes = panes;
  store.inputs = (json.inputs ?? {}) as Record<string, unknown>;
  store.peers = readPeers(json.__peers);
  const scopes = new Set<string>();
  for (const key of Object.keys(panes)) {
    scopes.add(paneScope(key));
  }
  store.producers = scopes.size;
  const n = Object.keys(panes).length;
  const where = timeline.source === 'live' ? '' : ' · recording';
  store.status =
    `${n} pane${n === 1 ? '' : 's'}` +
    (scopes.size > 1 ? ` · ${scopes.size} producers` : '') +
    where;
}

// Render the LIVE document at the current view: the latest version while
// following. A recording never enters here — its frames come from the worker.
function renderLive(): void {
  // Keep position pinned to the live edge while following, so pressing Play
  // restarts from the recording's start (position >= maxTs) rather than from
  // a stale position (often 0 = the Unix epoch, which looks stuck).
  timeline.position = timeline.maxTs;
  doc.checkoutToLatest();
  applyDoc(doc.toJSON());
}

// Rebuild the live change index and timeline bounds from the oplog. The live doc
// grows by small incremental frames, so this stays cheap on the main thread.
function rebuildLiveBounds(): void {
  const next: Mark[] = [];
  for (const [peer, changes] of doc.getAllChanges()) {
    for (const c of changes) {
      // A change with no timestamp cannot be placed on a time axis, and one that
      // slipped through would drag `minTs` back to the Unix epoch and flatten the
      // whole scrubber. Every writer here stamps milliseconds explicitly, so this
      // only ever skips a change some future writer forgot to stamp.
      if (c.timestamp > 0) next.push(markOf(String(c.peer ?? peer), c));
    }
  }
  sortMarks(next);
  liveMarks = next;
  timeline.changeCount = next.length;
  timeline.minTs = next.length ? next[0].ts : 0;
  timeline.maxTs = next.length ? next[next.length - 1].ts : 0;
  rebuildEdits();
}

// ----- edit history -------------------------------------------------------

// Turn the newest window of changes into history rows. Only changes not already
// in the cache are read back from the oplog, so a live frame costs one op read
// per new change rather than a full re-scan.
function rebuildEdits(): void {
  const window = liveMarks.slice(-EDIT_WINDOW);
  const next = new Map<string, EditRow>();
  const rows: EditRow[] = [];
  for (const mark of window) {
    const id = changeId(mark.peer, mark.start);
    let row = editCache.get(id);
    if (!row) row = readEdit(id, mark);
    next.set(id, row);
    rows.push(row);
  }
  // Drop rows that fell out of the window, so the cache tracks the panel rather
  // than growing for the life of the page.
  editCache = next;
  edits.rows = rows;
  edits.localPeer = doc.peerIdStr;
}

// Read one change's ops and reduce them to a row. A change is immutable once
// committed, so this is a pure function of the change -- except for the container
// paths, which are resolved against the CURRENT document: an edit to a pane that
// has since been deleted keeps its "what" and loses only its scroll target.
function readEdit(id: string, mark: Mark): EditRow {
  const base = {
    id,
    peer: mark.peer,
    start: mark.start,
    length: mark.length,
    counter: mark.counter,
    ts: mark.ts,
    lamport: mark.lamport,
  };
  try {
    const [change] = doc.exportJsonInIdSpan({
      peer: mark.peer,
      counter: mark.start,
      length: mark.length,
    });
    if (!change) return { ...base, what: `${mark.length} ops`, where: '', target: null, inputs: [] };
    return { ...base, ...summarize(change, (cid) => doc.getPathToContainer(cid)) };
  } catch (err) {
    // A change we cannot read must still appear: a gap in the history is worse
    // than a row that only says how big it was.
    console.warn('dashboard: could not read change', id, err);
    return { ...base, what: `${mark.length} ops`, where: '', target: null, inputs: [] };
  }
}

function onFrame(data: string): void {
  try {
    doc.import(b64ToBytes(data));
  } catch (err) {
    // A single malformed frame must not kill the listener; the next good update
    // (or a snapshot on reconnect) recovers the view.
    console.warn('dashboard: dropped malformed frame', err);
    return;
  }
  rebuildLiveBounds();
  if (pendingSeek !== null && timeline.minTs <= pendingSeek && pendingSeek <= timeline.maxTs) {
    const target = pendingSeek;
    pendingSeek = null;
    scrubTo(target);
    return;
  }
  // Following tracks live; a pinned (scrubbing) view stays put while the slider
  // max grows underneath it.
  if (timeline.following) renderLive();
}

export function connect(): void {
  if (es) return;
  timeline.source = 'live';
  timeline.following = true;
  timeline.seeking = false;
  doc = newDoc();
  liveMarks = [];
  editCache = new Map();
  edits.rows = [];
  edits.marked = null;
  edits.localPeer = doc.peerIdStr;
  registered = false;
  writes.error = null;
  writes.pending = 0;
  unsubscribeLocal?.();
  // Every LOCAL commit hands us the bytes to send. An `import` never fires this,
  // so the server broadcasting our own ops back cannot start a loop -- the echo
  // arrives as an import, loro applies it idempotently, and nothing is re-posted.
  unsubscribeLocal = doc.subscribeLocalUpdates((bytes) => {
    void postUpdate(bytes);
  });
  openStream();
  void refreshRecordings();
  applyHash();
}

// Open the `/events` stream against the CURRENT document. Split out from
// `connect` because a resync must re-read the server's snapshot WITHOUT throwing
// the document away: it may be holding local ops that have not landed yet.
function openStream(): void {
  es?.close();
  es = new EventSource('/events');
  es.addEventListener('open', () => {
    store.live = true;
  });
  es.addEventListener('error', () => {
    store.live = false;
    store.status = 'reconnecting…';
  });
  const ingest = (event: MessageEvent) => onFrame(event.data);
  es.addEventListener('snapshot', ingest as EventListener);
  es.addEventListener('update', ingest as EventListener);
}

// ----- timeline controls --------------------------------------------------

function stopClock(): void {
  if (raf) {
    cancelAnimationFrame(raf);
    raf = 0;
  }
  lastTick = 0;
}

function step(now: number): void {
  if (!timeline.playing) {
    raf = 0;
    return;
  }
  const dt = lastTick ? now - lastTick : 0;
  lastTick = now;
  timeline.position = Math.min(timeline.maxTs, timeline.position + dt * timeline.speed);
  if (timeline.position >= timeline.maxTs) {
    // Reached the end: snap back to following the latest (live tail, or the
    // recording's end). goLive() renders maxTs itself, so don't render here too
    // — a second seek to the same version would repeat the (costly) checkout.
    goLive();
    return;
  }
  renderAt(timeline.position);
  raf = requestAnimationFrame(step);
}

export function play(): void {
  if (timeline.changeCount <= 1) return;
  // Restart from the beginning when parked at the end.
  if (timeline.position >= timeline.maxTs) timeline.position = timeline.minTs;
  timeline.following = false;
  timeline.playing = true;
  lastTick = 0;
  if (!raf) raf = requestAnimationFrame(step);
}

export function pause(): void {
  timeline.playing = false;
  stopClock();
}

// Render the view at `ts`. Live checks out on the main thread (cheap); a
// recording hands the timestamp to the worker, which coalesces rapid requests and
// posts back the frame.
function renderAt(ts: number): void {
  if (timeline.source === 'live') {
    if (timeline.following) {
      renderLive();
    } else {
      // The live doc's history is small; a bounded checkout on the main thread is
      // fine. Reuse the recording-free path by checking out directly.
      checkoutLiveTo(ts);
    }
    return;
  }
  if (worker) {
    // During playback frames flow continuously; only flag "seeking" for a manual
    // scrub, so the indicator marks a deliberate jump rather than pulsing on
    // every animation frame.
    if (!timeline.playing) timeline.seeking = true;
    const req: RecordingRequest = { type: 'seek', ts };
    worker.postMessage(req);
  }
}

// Check the live doc out to the frontier at `ts` and read it into the store.
// Only used for a pinned live view; the live oplog is small so this is cheap.
function checkoutLiveTo(ts: number): void {
  const frontier = frontierAt(liveMarks, ts);
  if (frontier.length) doc.checkout(frontier);
  applyDoc(doc.toJSON());
}

export function scrubTo(ts: number): void {
  timeline.following = false;
  timeline.playing = false;
  stopClock();
  timeline.position = Math.max(timeline.minTs, Math.min(timeline.maxTs, ts));
  renderAt(timeline.position);
}

export function goLive(): void {
  timeline.following = true;
  timeline.playing = false;
  stopClock();
  timeline.position = timeline.maxTs;
  renderAt(timeline.position);
}

export function setSpeed(speed: number): void {
  timeline.speed = speed;
}

// The reference time for a pane's age: wall-clock while following live, else the
// scrubbed-to moment, so a card shows its age as of the replayed instant.
export function referenceMs(): number {
  if (timeline.source === 'live' && timeline.following) return Date.now();
  return timeline.position || timeline.maxTs;
}

// ----- the write path -----------------------------------------------------

// Whether this browser can write right now.
//
// A detached document is read-only here on purpose. Scrubbing the live history or
// replaying a recording checks the document out to a past version; an edit
// committed against that version would branch the history off a point in the past
// and nothing would ever merge it back. So the controls refuse instead, and say
// why.
//
// The listener check is the other half: `?demo` seeds `store.panes` directly and
// never calls `connect()`, so there is no document behind the view. Writing there
// would commit into an empty document, find no subscriber to post it, and then
// re-render the store from that empty document -- wiping the demo.
export function canEdit(): boolean {
  return (
    unsubscribeLocal !== null &&
    timeline.source === 'live' &&
    timeline.following &&
    !doc.isDetached()
  );
}

// This browser's peer id, as the decimal string loro uses (a u64 does not fit a
// JS number, so it is never a number here).
export function localPeerId(): string {
  return doc.peerIdStr;
}

// The answer currently recorded for an input, or undefined.
export function inputValue(key: string): unknown {
  return store.inputs[key];
}

// Record an answer in the document.
//
// The click IS the edit. There is no message, no payload schema and no acking:
// the value is written into the `inputs` root, which is last-write-wins per key,
// so clicking the same answer twice produces the same document and two viewers
// answering at once converge on one value instead of on a race.
export function setInput(key: string, value: JsonValue): boolean {
  if (!canEdit()) return false;
  try {
    registerSelf();
    doc.getMap('inputs').set(key, value);
    // Milliseconds, matching the hub's `set_next_commit_timestamp(now_ms())`, so a
    // viewer's edit lands on the same timeline axis as a producer's tick. Leaving
    // it to loro would stamp seconds and drop the change at the epoch end of the
    // scrubber.
    doc.commit({ timestamp: Date.now(), origin: 'viewer' });
  } catch (err) {
    writes.error = `could not record the answer: ${String(err)}`;
    return false;
  }
  // Show it immediately rather than waiting for the server to echo it back: the
  // edit is already in our document, and the echo is idempotent.
  rebuildLiveBounds();
  renderLive();
  return true;
}

// Replace a shared note draft's text (an agent compose box) with `value`.
//
// The draft is a mergeable LoroText the hub declares beside every terminal pane
// (`<scope>\x1f<pane>\x1fcompose`), so the container is already in the snapshot
// this browser imported. `update` diffs the current content against `value` into
// minimal insert/delete ops, which is what lets two people type into one draft
// at once and both sentences survive. The commit travels the same /apply path as
// `setInput`, 409-rebase included.
//
// Refuses (false) when the field was never declared or is not a text container:
// creating it here would race a concurrent viewer's creation and drop whatever
// they had typed (see Hub::declare_note), so an undeclared draft stays read-only.
export function setNoteText(key: string, value: string): boolean {
  if (!canEdit()) return false;
  const container = doc.getMap('inputs').get(key);
  if (!(container instanceof LoroText)) return false;
  try {
    registerSelf();
    container.update(value);
    doc.commit({ timestamp: Date.now(), origin: 'viewer' });
  } catch (err) {
    writes.error = `could not update the draft: ${String(err)}`;
    return false;
  }
  rebuildLiveBounds();
  renderLive();
  return true;
}

const VIEWER_LABEL_KEY = 'dash-viewer-label';

function viewerLabel(): string {
  try {
    return localStorage.getItem(VIEWER_LABEL_KEY) || 'viewer';
  } catch {
    return 'viewer';
  }
}

// Announce this browser in `__peers` so other viewers can attribute its edits.
//
// Deferred to the first real edit and folded into that same commit: a viewer who
// only reads writes nothing at all, and one who answers adds a row to the history
// for the answer, not a second row for having shown up.
function registerSelf(): void {
  if (registered) return;
  registered = true;
  doc.getMap('__peers').set(doc.peerIdStr, { kind: 'human', label: viewerLabel() });
}

// Updates waiting to go out. One drain loop owns the queue so two fast clicks
// arrive in order and a 409 recovery cannot interleave with a fresh post.
const outbox: Uint8Array[] = [];
let draining = false;

async function postUpdate(bytes: Uint8Array): Promise<void> {
  outbox.push(bytes);
  writes.pending = outbox.length;
  if (draining) return;
  draining = true;
  try {
    while (outbox.length) {
      await send(outbox[0]);
      outbox.shift();
      writes.pending = outbox.length;
    }
  } finally {
    draining = false;
    writes.pending = outbox.length;
  }
}

// POST raw update bytes. The outbound stream base64s only because SSE is a text
// protocol; a request body has no such constraint, so this is octet-stream.
async function apply(body: Uint8Array): Promise<Response | null> {
  try {
    return await fetch('/apply', {
      method: 'POST',
      headers: { 'content-type': 'application/octet-stream' },
      // A fresh view over the same buffer: `fetch` wants an ArrayBuffer-backed
      // body and loro hands back a Uint8Array over WASM memory.
      body: body.slice().buffer as ArrayBuffer,
    });
  } catch (err) {
    writes.error = `the dashboard is unreachable: ${String(err)}`;
    return null;
  }
}

async function send(update: Uint8Array): Promise<void> {
  const resp = await apply(update);
  if (!resp) return;
  if (resp.status === 204) {
    writes.error = null;
    return;
  }
  if (resp.status === 409) {
    await rebase();
    return;
  }
  // 400 and anything else: the server could not decode or would not take it. Say
  // so. Dropping it silently would leave a human believing an answer they gave
  // was recorded when the only copy of it is in this tab.
  const detail = await resp.text().catch(() => '');
  writes.error = `the dashboard refused the edit (${resp.status})${detail ? `: ${detail}` : ''}`;
}

// Recover from a 409.
//
// 409 means our update is built on ops the server's document has never seen, so
// there is nothing for it to attach to -- which happens when the aggregator
// restarted with a fresh document under a still-open page. Reposting the same
// bytes would 409 forever. So: reopen `/events` (its first frame is a snapshot,
// which merges into our document rather than replacing it, keeping the local ops
// that have not landed), then post a full SNAPSHOT of ours. A snapshot depends on
// nothing, so the server can always apply it.
async function rebase(): Promise<void> {
  openStream();
  let snapshot: Uint8Array;
  try {
    snapshot = doc.export({ mode: 'snapshot' });
  } catch (err) {
    writes.error = `could not rebuild this edit for the server: ${String(err)}`;
    return;
  }
  const resp = await apply(snapshot);
  if (!resp) return;
  if (resp.status === 204) {
    writes.error = null;
    return;
  }
  const detail = await resp.text().catch(() => '');
  writes.error = `the dashboard could not merge this edit (${resp.status})${detail ? `: ${detail}` : ''} — reload to resync`;
}

// Re-offer everything this tab holds that the server may not. Used by the error
// banner: a snapshot always applies, so this is the one retry that can work.
export function retryWrites(): void {
  writes.error = null;
  void rebase();
}

export function dismissWriteError(): void {
  writes.error = null;
}

// ----- edit marks ---------------------------------------------------------

// Mark one edit as the one being looked at. The document draws a marker where it
// landed; nothing is dimmed and nothing moves on its own.
export function markEdit(id: string | null): void {
  edits.marked = id;
}

// The newest edit that answered `key`, for a control that wants to name who
// decided. Null when the answer predates the history window.
export function answeredBy(key: string): EditRow | null {
  for (let i = edits.rows.length - 1; i >= 0; i--) {
    if (edits.rows[i].inputs.includes(key)) return edits.rows[i];
  }
  return null;
}

// ----- recordings ---------------------------------------------------------

export async function refreshRecordings(): Promise<void> {
  try {
    const resp = await fetch('/recordings');
    if (resp.ok) timeline.recordings = (await resp.json()) as RecordingInfo[];
  } catch {
    // No recordings endpoint (an old aggregator) just means no replay list.
  }
}

function ensureWorker(): Worker {
  if (worker) return worker;
  worker = new RecordingWorker();
  worker.onmessage = (event: MessageEvent<RecordingResponse>) => {
    const msg = event.data;
    // Drop replies for a recording we have since left or switched away from.
    if (msg.id !== activeRecordingId) return;
    if (msg.type === 'loaded') {
      timeline.minTs = msg.minTs;
      timeline.maxTs = msg.maxTs;
      timeline.changeCount = msg.changeCount;
      timeline.position = msg.startTs;
      timeline.seeking = false;
      applyDoc(msg.doc);
      // A deep link opened this recording to jump to a shared moment; now that
      // the bounds are known, honour it.
      if (recordingSeekOnLoad !== null) {
        const at = recordingSeekOnLoad;
        recordingSeekOnLoad = null;
        scrubTo(at);
      }
    } else if (msg.type === 'frame') {
      timeline.seeking = false;
      applyDoc(msg.doc);
    } else {
      timeline.seeking = false;
      console.warn('dashboard: recording replay failed', msg.message);
    }
  };
  return worker;
}

// Load a recording into the replay worker. `seekTo`, when given, is the moment to
// scrub to once the oplog has imported (used by a `#t=` deep link).
export async function loadRecording(id: string, seekTo?: number): Promise<void> {
  recordingSeekOnLoad = seekTo ?? null;
  let bytes: ArrayBuffer;
  try {
    const resp = await fetch(`/recording/${encodeURIComponent(id)}`);
    if (!resp.ok) return;
    bytes = await resp.arrayBuffer();
  } catch {
    return;
  }
  if (es) {
    es.close();
    es = null;
  }
  store.live = false;
  stopClock();
  timeline.source = id;
  // Open a recording parked at its start, paused, ready to play. The worker
  // reports the real bounds once it has imported the oplog.
  timeline.following = false;
  timeline.playing = false;
  timeline.seeking = true;
  activeRecordingId = id;
  const w = ensureWorker();
  const req: RecordingRequest = { type: 'load', id, bytes };
  // Transfer the buffer so a large recording is not copied into the worker.
  w.postMessage(req, [bytes]);
}

export function leaveRecording(): void {
  if (timeline.source === 'live') return;
  stopClock();
  activeRecordingId = '';
  timeline.seeking = false;
  // Drop any `#rec=`/`#t=` deep link first: `connect()` re-runs `applyHash`,
  // which would otherwise reload the very recording we are leaving.
  if (location.hash) history.replaceState(null, '', location.pathname + location.search);
  es = null; // connect() guards on a live handle; ensure it reconnects.
  connect();
  goLive();
}

// ----- sharing ------------------------------------------------------------

export function shareUrl(): string {
  const base = location.origin + location.pathname;
  const at = Math.round(timeline.following ? timeline.maxTs : timeline.position);
  if (timeline.source !== 'live') return `${base}#rec=${encodeURIComponent(timeline.source)}&t=${at}`;
  return `${base}#t=${at}`;
}

function applyHash(): void {
  const params = new URLSearchParams(location.hash.replace(/^#/, ''));
  const rec = params.get('rec');
  const t = params.get('t');
  if (rec) {
    void loadRecording(rec, t ? Number(t) : undefined);
  } else if (t) {
    // A live deep link: seek once the streamed history reaches that moment.
    pendingSeek = Number(t);
  }
}
