// Live pane state plus the replay timeline. The resources live in their owning
// process; the browser only reads, so the document has one editor per scope and
// we never write back.
//
// The hub records a millisecond timestamp on every change, so the imported
// document is a full recording: we keep the whole oplog, list its changes, and
// check the document out to any past version to replay it. `store` is the
// rendered pane set at the current view; `timeline` is the scrubber state.
// Switching to a saved recording swaps the document for one fetched over HTTP,
// so live and replay share one rendering path.
import { LoroDoc, type ChangeMeta, type OpId } from 'https://esm.sh/loro-crdt@1';
import { paneScope, SCOPE_SEP } from './scope.ts';
import type { PaneRecord } from './types';

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
export const timeline = $state({
  source: 'live' as string,
  following: true,
  playing: false,
  speed: 1,
  minTs: 0,
  maxTs: 0,
  position: 0,
  changeCount: 0,
  recordings: [] as RecordingInfo[],
});

// One change reduced to what scrubbing needs: the id of its last op (the version
// to check out to land exactly after it) and its timestamp.
interface Mark {
  peer: string;
  counter: number;
  ts: number;
  lamport: number;
}

let doc = new LoroDoc();
let marks: Mark[] = [];
let es: EventSource | null = null;
let raf = 0;
let lastTick = 0;
// A `#t=` deep link to apply once the live history covers it.
let pendingSeek: number | null = null;

function b64ToBytes(b64: string): Uint8Array {
  const bin = atob(b64);
  const out = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
  return out;
}

// Rebuild the change index from the oplog and refresh the timeline bounds. The
// aggregator is the sole editor, so the marks sort cleanly by time then lamport.
function rebuildIndex(): void {
  const next: Mark[] = [];
  for (const [peer, changes] of doc.getAllChanges()) {
    for (const c of changes) {
      next.push({ peer: String(peer), counter: c.counter + c.length - 1, ts: c.timestamp, lamport: c.lamport });
    }
  }
  next.sort((a, b) => a.ts - b.ts || a.lamport - b.lamport);
  marks = next;
  timeline.changeCount = next.length;
  timeline.minTs = next.length ? next[0].ts : 0;
  timeline.maxTs = next.length ? next[next.length - 1].ts : 0;
}

// The version (frontier) at or before `ts`. Single-editor history means the last
// mark at or before `ts` fully describes the state, so its op id is the frontier.
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

// Read the current document view into `store`. Called after every checkout, so
// the same path renders the live latest and any scrubbed-to past version.
function readPanes(): void {
  const panes = (doc.toJSON().panes ?? {}) as Record<string, PaneRecord>;
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

// Render the document at the current timeline view: the latest version while
// following, else the frontier at `position`.
function render(): void {
  if (timeline.following) {
    // Keep position pinned to the live edge while following, so pressing Play
    // restarts from the recording's start (position >= maxTs) rather than from
    // a stale position (often 0 = the Unix epoch, which looks stuck).
    timeline.position = timeline.maxTs;
    doc.checkoutToLatest();
  } else {
    const frontier = frontierAt(timeline.position);
    if (frontier.length) doc.checkout(frontier);
  }
  readPanes();
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
  rebuildIndex();
  if (pendingSeek !== null && timeline.minTs <= pendingSeek && pendingSeek <= timeline.maxTs) {
    const target = pendingSeek;
    pendingSeek = null;
    scrubTo(target);
    return;
  }
  // Following tracks live; a pinned (scrubbing) view stays put while the slider
  // max grows underneath it.
  if (timeline.following) render();
}

export function connect(): void {
  if (es) return;
  timeline.source = 'live';
  timeline.following = true;
  doc = new LoroDoc();
  marks = [];
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
  render();
  if (timeline.position >= timeline.maxTs) {
    // Reached the end: snap back to following the latest (live tail, or the
    // recording's end).
    goLive();
    return;
  }
  raf = requestAnimationFrame(step);
}

export function play(): void {
  if (!marks.length) return;
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

export function scrubTo(ts: number): void {
  timeline.following = false;
  timeline.playing = false;
  stopClock();
  timeline.position = Math.max(timeline.minTs, Math.min(timeline.maxTs, ts));
  render();
}

export function goLive(): void {
  timeline.following = true;
  timeline.playing = false;
  stopClock();
  timeline.position = timeline.maxTs;
  render();
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

export async function loadRecording(id: string): Promise<void> {
  let bytes: Uint8Array;
  try {
    const resp = await fetch(`/recording/${encodeURIComponent(id)}`);
    if (!resp.ok) return;
    bytes = new Uint8Array(await resp.arrayBuffer());
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
  doc = new LoroDoc();
  try {
    doc.import(bytes);
  } catch (err) {
    console.warn('dashboard: failed to load recording', err);
    return;
  }
  rebuildIndex();
  // Open a recording parked at its start, paused, ready to play.
  timeline.following = false;
  timeline.playing = false;
  timeline.position = timeline.minTs;
  render();
}

export function leaveRecording(): void {
  if (timeline.source === 'live') return;
  stopClock();
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
    void loadRecording(rec).then(() => {
      if (t) scrubTo(Number(t));
    });
  } else if (t) {
    // A live deep link: seek once the streamed history reaches that moment.
    pendingSeek = Number(t);
  }
}
