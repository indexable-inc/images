// Live pane state plus the replay timeline. The resources live in their owning
// process; the browser only reads, so the document has one editor per scope and
// we never write back.
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
import { LoroDoc } from 'https://esm.sh/loro-crdt@1';
import { paneScope, SCOPE_SEP } from './scope.ts';
import type { PaneRecord } from './types';
import RecordingWorker from './recording-worker.ts?worker&inline';
import type { RecordingRequest, RecordingResponse } from './recording-worker.ts';

export { SCOPE_SEP };

export const store = $state({
  panes: {} as Record<string, PaneRecord>,
  producers: 0,
  live: false,
  status: 'connecting',
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

// One change reduced to what a pinned-live scrub needs: the id of its last op
// (the version to check out to land just after it) and its timestamp.
interface Mark {
  peer: string;
  counter: number;
  ts: number;
  lamport: number;
}

let doc = new LoroDoc();
// The live doc's change index, rebuilt on each frame. Only the live path uses it;
// a recording's index lives in the worker with its document.
let liveMarks: Mark[] = [];
let es: EventSource | null = null;
let raf = 0;
let lastTick = 0;
// A `#t=` deep link to apply once the live history covers it.
let pendingSeek: number | null = null;

// The version (frontier) at or before `ts`: the last mark at or before it. The
// aggregator is the sole editor, so that mark fully describes the state there.
function frontierAt(marks: Mark[], ts: number): { peer: string; counter: number }[] {
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

// Read a pane set into `store`, deriving the producer count and status line. The
// live path and the worker's replay frames both funnel through here so they
// render identically.
function applyPanes(panes: Record<string, PaneRecord>): void {
  store.panes = panes;
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
  applyPanes((doc.toJSON().panes ?? {}) as Record<string, PaneRecord>);
}

// Rebuild the live change index and timeline bounds from the oplog. The live doc
// grows by small incremental frames, so this stays cheap on the main thread.
function rebuildLiveBounds(): void {
  const next: Mark[] = [];
  for (const [peer, changes] of doc.getAllChanges()) {
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
  liveMarks = next;
  timeline.changeCount = next.length;
  timeline.minTs = next.length ? next[0].ts : 0;
  timeline.maxTs = next.length ? next[next.length - 1].ts : 0;
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
  doc = new LoroDoc();
  liveMarks = [];
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
  void refreshRecordings();
  applyHash();
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
  applyPanes((doc.toJSON().panes ?? {}) as Record<string, PaneRecord>);
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
      applyPanes(msg.panes);
      // A deep link opened this recording to jump to a shared moment; now that
      // the bounds are known, honour it.
      if (recordingSeekOnLoad !== null) {
        const at = recordingSeekOnLoad;
        recordingSeekOnLoad = null;
        scrubTo(at);
      }
    } else if (msg.type === 'frame') {
      timeline.seeking = false;
      applyPanes(msg.panes);
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
