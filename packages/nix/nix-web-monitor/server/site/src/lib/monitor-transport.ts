/// WebSocket client for the live delta stream.
///
/// The server pushes a `reset` seed then incremental `Delta`s as msgpack
/// payloads, one delta per binary WebSocket frame. WebSocket preserves message
/// boundaries, so there is no length prefix to reassemble: we decode each frame,
/// validate it, and fold it into a working model, projecting a fresh
/// `MonitorSnapshot` to the caller. The page is served over plain HTTP on the
/// same origin, so the socket opens `ws://` with no TLS or certificate dance.

import { decode } from '@msgpack/msgpack';
import * as v from 'valibot';
import {
  deltaSchema,
  type ActivityNode,
  type BuildNode,
  type ConnectionStatus,
  type Delta,
  type DerivationEdge,
  type LogEntry,
  type MonitorSnapshot
} from '$lib/types';

type SnapshotHandler = (snapshot: MonitorSnapshot) => void;
type StatusHandler = (status: ConnectionStatus) => void;

/// Mirror the server's retention caps so a long run cannot grow these without
/// bound. The UI only renders the tail anyway; older entries fall off the head.
const LOG_RETAIN = 5_000;
const ERROR_RETAIN = 2_000;

/// Mutable accumulation of the snapshot, keyed for O(1) upserts. Kept private to
/// this module; callers only ever see the immutable projection.
type Working = {
  command: string;
  activities: Map<number, ActivityNode>;
  builds: Map<string, BuildNode>;
  logs: LogEntry[];
  errors: string[];
  progress: MonitorSnapshot['progress'];
  optimise: MonitorSnapshot['optimise'];
  daemon: MonitorSnapshot['daemon'];
  activation: MonitorSnapshot['activation'];
  diff: MonitorSnapshot['diff'];
  expected: Record<string, number>;
  dependencies: DerivationEdge[];
  rootCauses: string[];
  rebuildReasons: Record<string, string>;
  exitCode: number | null;
  finished: boolean;
};

function createWorking(): Working {
  return {
    command: '',
    activities: new Map(),
    builds: new Map(),
    logs: [],
    errors: [],
    progress: null,
    optimise: { filesLinked: 0, bytesFreed: 0 },
    daemon: {
      tracing: false,
      status: '',
      workers: [],
      ops: { link: 0, rename: 0, open: 0, write: 0, fsync: 0, stat: 0, unlink: 0, other: 0 },
      opsPerSec: 0,
      currentPath: null,
      hotPaths: []
    },
    activation: { active: false, command: '', steps: [], status: '' },
    diff: null,
    expected: {},
    dependencies: [],
    rootCauses: [],
    rebuildReasons: {},
    exitCode: null,
    finished: false
  };
}

/// Fold one delta into the working model. A `reset` replaces everything; the
/// rest patch in place. `logsAppend` mirrors the server's per-build `logCount`
/// so the hot log path needs no `buildUpsert`.
export function applyDelta(working: Working, delta: Delta): Working {
  switch (delta.type) {
    case 'reset':
      return fromSnapshot(delta.snapshot);
    case 'buildUpsert':
      working.builds.set(delta.build.derivation, delta.build);
      return working;
    case 'activityUpsert':
      working.activities.set(delta.activity.id, delta.activity);
      return working;
    case 'logsAppend':
      for (const entry of delta.entries) {
        working.logs.push(entry);
        bumpLogCount(working, entry);
      }
      capHead(working.logs, LOG_RETAIN);
      return working;
    case 'progressSet':
      working.progress = delta.progress;
      return working;
    case 'optimiseSet':
      working.optimise = delta.optimise;
      return working;
    case 'daemonSet':
      working.daemon = delta.daemon;
      return working;
    case 'activationSet':
      working.activation = delta.activation;
      return working;
    case 'diffSet':
      working.diff = delta.diff;
      return working;
    case 'expectedSet':
      working.expected = { ...working.expected, [delta.name]: delta.value };
      return working;
    case 'errorAppend':
      working.errors.push(delta.message);
      capHead(working.errors, ERROR_RETAIN);
      return working;
    case 'dependenciesSet':
      working.dependencies = delta.edges;
      return working;
    case 'rootCausesSet':
      working.rootCauses = delta.derivations;
      return working;
    case 'rebuildReasonSet':
      working.rebuildReasons = { ...working.rebuildReasons, [delta.derivation]: delta.reason };
      return working;
    case 'finished':
      working.exitCode = delta.exitCode;
      working.finished = true;
      return working;
  }
}

function fromSnapshot(snapshot: MonitorSnapshot): Working {
  return {
    command: snapshot.command,
    activities: new Map(snapshot.activities.map((activity) => [activity.id, activity])),
    builds: new Map(snapshot.builds.map((build) => [build.derivation, build])),
    logs: [...snapshot.logs],
    errors: [...snapshot.errors],
    progress: snapshot.progress,
    optimise: snapshot.optimise,
    daemon: snapshot.daemon,
    activation: snapshot.activation,
    diff: snapshot.diff,
    expected: { ...snapshot.expected },
    dependencies: snapshot.dependencies,
    rootCauses: snapshot.rootCauses,
    rebuildReasons: { ...snapshot.rebuildReasons },
    exitCode: snapshot.exitCode,
    finished: snapshot.finished
  };
}

/// Match the server's `push_log`: a log line on a build's activity bumps that
/// build's `logCount`, independent of log retention so the count never regresses.
function bumpLogCount(working: Working, entry: LogEntry): void {
  if (entry.activityId === null) return;
  const activity = working.activities.get(entry.activityId);
  if (activity?.build == null) return;
  const build = working.builds.get(activity.build);
  if (build === undefined) return;
  working.builds.set(activity.build, { ...build, logCount: build.logCount + 1 });
}

function capHead(items: unknown[], max: number): void {
  if (items.length > max) items.splice(0, items.length - max);
}

/// Project the working model into a fresh immutable snapshot. Builds sort by
/// derivation and activities by id for a stable order; the UI applies its own
/// display ordering on top.
export function projectSnapshot(working: Working): MonitorSnapshot {
  return {
    command: working.command,
    activities: [...working.activities.values()].toSorted((a, b) => a.id - b.id),
    builds: [...working.builds.values()].toSorted((a, b) =>
      a.derivation.localeCompare(b.derivation)
    ),
    logs: [...working.logs],
    errors: [...working.errors],
    progress: working.progress,
    optimise: working.optimise,
    daemon: working.daemon,
    activation: working.activation,
    diff: working.diff,
    expected: { ...working.expected },
    dependencies: working.dependencies,
    rootCauses: working.rootCauses,
    rebuildReasons: { ...working.rebuildReasons },
    exitCode: working.exitCode,
    finished: working.finished
  };
}

function decodeDelta(payload: Uint8Array): Delta | null {
  try {
    const result = v.safeParse(deltaSchema, decode(payload));
    return result.success ? result.output : null;
  } catch {
    return null;
  }
}

/// How long a session may be down before the indicator leaves `live`. A drop
/// the client reconnects through within this window never flickers the status,
/// which is what kept the old single-shot driver oscillating live/error on a
/// lossy link.
const GRACE_MS = 1_200;
/// Reconnect backoff bounds. The floor keeps a flapping session from busy-
/// looping; the ceiling keeps a hard-down endpoint from being hammered while
/// still recovering within a few seconds once it returns.
const BACKOFF_MIN_MS = 250;
const BACKOFF_MAX_MS = 5_000;

/// Open the WebSocket session and drive the snapshot/status callbacks until the
/// returned disposer is called or the monitored run finishes. A dropped session
/// reconnects with backoff.
export function openMonitorEvents(onSnapshot: SnapshotHandler, onStatus: StatusHandler): () => void {
  onStatus('connecting');

  // Cancellation flips with the disposer from outside the async flow. Read
  // through `isAborted()` so the type-aware lint re-evaluates it at each
  // await-point instead of narrowing it to a constant after the first guard.
  const aborted = new AbortController();
  const isAborted = (): boolean => aborted.signal.aborted;
  let socket: WebSocket | null = null;
  // `live` once a session has ever come up: it picks the degraded label
  // (`reconnecting` vs the initial `error`) and is the signal that a drop is
  // transient rather than a cold start against a server that is not up yet.
  let everLive = false;
  // Reconnect counter, reset to zero each time a socket opens so the backoff
  // grows only across consecutive failures.
  let attempt = 0;
  let degradeTimer: ReturnType<typeof setTimeout> | null = null;

  // Hold the visible status on its pre-drop value until the link has been down
  // for `GRACE_MS`; only then surface the degraded label. A reconnect inside
  // the window cancels the timer, so brief blips stay invisible.
  const armDegrade = (): void => {
    if (degradeTimer !== null) return;
    degradeTimer = setTimeout(() => {
      degradeTimer = null;
      if (!isAborted()) onStatus(everLive ? 'reconnecting' : 'error');
    }, GRACE_MS);
  };
  const clearDegrade = (): void => {
    if (degradeTimer === null) return;
    clearTimeout(degradeTimer);
    degradeTimer = null;
  };

  void loop();

  async function loop(): Promise<void> {
    while (!isAborted()) {
      let finished = false;
      try {
        finished = await runSession();
      } catch {
        // Fall through to the reconnect/degrade handling below.
      }
      if (isAborted()) return;
      // A clean end carrying the `finished` delta means the run is over; stop.
      // Any other close (errored or dropped mid-run) is treated as a drop.
      if (finished) {
        onStatus('closed');
        return;
      }
      armDegrade();
      attempt += 1;
      await delay(Math.min(BACKOFF_MAX_MS, BACKOFF_MIN_MS * 2 ** (attempt - 1)));
    }
  }

  /// Run one WebSocket session, folding each binary frame into a snapshot.
  /// Resolves `true` when the socket closed after a `finished` delta (run
  /// complete) and `false` on any other close, which the caller reconnects
  /// through. Resolves rather than rejects on error: a failed connect is just a
  /// drop, and `onclose` always follows `onerror`, so the outcome settles once.
  function runSession(): Promise<boolean> {
    return new Promise((resolve) => {
      const scheme = location.protocol === 'https:' ? 'wss' : 'ws';
      const ws = new WebSocket(`${scheme}://${location.host}/ws`);
      socket = ws;
      ws.binaryType = 'arraybuffer';

      // `applyDelta` mutates in place for incremental deltas but returns a fresh
      // object for `reset` (the seed), so the binding must be reassignable: the
      // seed carries the builds/activities for a page loaded mid-build, and
      // dropping the return would silently discard them (logs survived only
      // because their delta mutates in place).
      let working = createWorking();
      let frameDue = false;
      let sawFinished = false;
      // Coalesce bursts of deltas into one snapshot per animation frame. The
      // terminal snapshot is delivered either by the `finished` branch's
      // explicit flush or by the last scheduled frame, so no trailing flush is
      // needed once the socket closes.
      const flush = (): void => {
        frameDue = false;
        if (!isAborted()) onSnapshot(projectSnapshot(working));
      };
      const schedule = (): void => {
        if (frameDue) return;
        frameDue = true;
        requestAnimationFrame(flush);
      };

      ws.onopen = (): void => {
        attempt = 0;
        everLive = true;
        clearDegrade();
        if (!isAborted()) onStatus('live');
      };
      ws.onmessage = (event: MessageEvent): void => {
        const delta = decodeDelta(new Uint8Array(event.data as ArrayBuffer));
        if (delta === null) return;
        working = applyDelta(working, delta);
        if (delta.type === 'finished') {
          sawFinished = true;
          flush();
          ws.close();
          return;
        }
        schedule();
      };
      // A transport error always pairs with a `close` event; let `onclose`
      // settle the result so it resolves exactly once.
      ws.onerror = (): void => {};
      ws.onclose = (): void => {
        resolve(sawFinished);
      };
    });
  }

  /// Backoff sleep that resolves early when the disposer aborts, so teardown
  /// does not wait out a pending reconnect delay.
  function delay(ms: number): Promise<void> {
    return new Promise((resolve) => {
      const timer = setTimeout(resolve, ms);
      aborted.signal.addEventListener(
        'abort',
        () => {
          clearTimeout(timer);
          resolve();
        },
        { once: true }
      );
    });
  }

  return () => {
    aborted.abort();
    clearDegrade();
    socket?.close();
  };
}
