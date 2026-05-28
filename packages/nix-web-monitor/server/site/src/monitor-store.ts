import { LoroDoc } from 'loro-crdt';
import {
  EMPTY_SNAPSHOT,
  type ActivityNode,
  type ActivityProgress,
  type BuildNode,
  type ConnectionStatus,
  type LogEntry,
  type MonitorSnapshot
} from './types';

type SnapshotHandler = (snapshot: MonitorSnapshot) => void;
type StatusHandler = (status: ConnectionStatus) => void;

const doc = new LoroDoc();
const root = doc.getMap('monitor');

export function rememberSnapshot(snapshot: MonitorSnapshot): MonitorSnapshot {
  root.set('snapshot', snapshot);
  doc.commit();
  return snapshot;
}

export function openMonitorEvents(onSnapshot: SnapshotHandler, onStatus: StatusHandler): () => void {
  onStatus('connecting');
  const events = new EventSource('/api/events');

  events.addEventListener('open', () => {
    onStatus('live');
  });

  events.addEventListener('snapshot', (event) => {
    const snapshot = parseSnapshotPayload(event);
    if (snapshot === null) {
      onStatus('error');
      return;
    }
    onSnapshot(rememberSnapshot(snapshot));
    onStatus(snapshot.finished ? 'closed' : 'live');
  });

  events.addEventListener('monitor-error', () => {
    onStatus('error');
  });

  events.addEventListener('error', () => {
    onStatus('error');
  });

  return () => {
    events.close();
  };
}

export async function fetchInitialSnapshot(): Promise<MonitorSnapshot> {
  const response = await fetch('/api/snapshot', { cache: 'no-store' });
  if (!response.ok) return EMPTY_SNAPSHOT;
  const parsed = parseSnapshot(await response.json());
  return parsed ?? EMPTY_SNAPSHOT;
}

function parseSnapshotPayload(event: Event): MonitorSnapshot | null {
  if (!(event instanceof MessageEvent) || typeof event.data !== 'string') return null;
  try {
    return parseSnapshot(JSON.parse(event.data));
  } catch {
    return null;
  }
}

function parseSnapshot(value: unknown): MonitorSnapshot | null {
  if (!isRecord(value)) return null;
  const activities = parseArray(value.activities, parseActivity);
  const builds = parseArray(value.builds, parseBuild);
  const logs = parseArray(value.logs, parseLog);
  const messages = parseStringArray(value.messages);
  const errors = parseStringArray(value.errors);
  const expected = parseNumberRecord(value.expected);
  if (
    activities === null ||
    builds === null ||
    logs === null ||
    messages === null ||
    errors === null ||
    expected === null ||
    !isNullableNumber(value.exitCode) ||
    typeof value.finished !== 'boolean'
  ) {
    return null;
  }

  return {
    activities,
    builds,
    logs,
    messages,
    errors,
    progress: parseProgress(value.progress),
    expected,
    exitCode: value.exitCode,
    finished: value.finished
  };
}

function parseActivity(value: unknown): ActivityNode | null {
  if (!isRecord(value)) return null;
  const activityType = value.activityType;
  if (
    !isRecord(activityType) ||
    !isNumber(activityType.code) ||
    typeof activityType.name !== 'string' ||
    !isNumber(value.id) ||
    !isNullableNumber(value.parent) ||
    typeof value.text !== 'string' ||
    !isNullableString(value.phase) ||
    !isActivityStatus(value.status) ||
    !isNumber(value.startedTick) ||
    !isNullableNumber(value.stoppedTick) ||
    !isNullableString(value.build)
  ) {
    return null;
  }

  return {
    id: value.id,
    parent: value.parent,
    activityType: { code: activityType.code, name: activityType.name },
    text: value.text,
    phase: value.phase,
    progress: parseProgress(value.progress),
    status: value.status,
    startedTick: value.startedTick,
    stoppedTick: value.stoppedTick,
    build: value.build
  };
}

function parseBuild(value: unknown): BuildNode | null {
  if (!isRecord(value)) return null;
  if (
    typeof value.derivation !== 'string' ||
    !isNumber(value.activityId) ||
    !isNullableString(value.host) ||
    !isNullableString(value.phase) ||
    !isBuildStatus(value.status) ||
    !isNumber(value.logCount)
  ) {
    return null;
  }

  return {
    derivation: value.derivation,
    activityId: value.activityId,
    host: value.host,
    phase: value.phase,
    status: value.status,
    logCount: value.logCount
  };
}

function parseLog(value: unknown): LogEntry | null {
  if (!isRecord(value)) return null;
  if (!isNumber(value.index) || !isNullableNumber(value.activityId) || typeof value.text !== 'string') {
    return null;
  }
  return { index: value.index, activityId: value.activityId, text: value.text };
}

function parseProgress(value: unknown): ActivityProgress | null {
  if (value === null) return null;
  if (!isRecord(value)) return null;
  if (
    !isNumber(value.done) ||
    !isNumber(value.expected) ||
    !isNumber(value.running) ||
    !isNumber(value.failed)
  ) {
    return null;
  }
  return {
    done: value.done,
    expected: value.expected,
    running: value.running,
    failed: value.failed
  };
}

function parseArray<T>(value: unknown, parse: (item: unknown) => T | null): T[] | null {
  if (!Array.isArray(value)) return null;
  const parsed: T[] = [];
  for (const item of value) {
    const next = parse(item);
    if (next === null) return null;
    parsed.push(next);
  }
  return parsed;
}

function parseStringArray(value: unknown): string[] | null {
  if (!Array.isArray(value)) return null;
  return value.every((item): item is string => typeof item === 'string') ? value : null;
}

function parseNumberRecord(value: unknown): Record<string, number> | null {
  if (!isRecord(value)) return null;
  const result: Record<string, number> = {};
  for (const [key, item] of Object.entries(value)) {
    if (!isNumber(item)) return null;
    result[key] = item;
  }
  return result;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null;
}

function isNumber(value: unknown): value is number {
  return typeof value === 'number' && Number.isFinite(value);
}

function isNullableNumber(value: unknown): value is number | null {
  return value === null || isNumber(value);
}

function isNullableString(value: unknown): value is string | null {
  return value === null || typeof value === 'string';
}

function isActivityStatus(value: unknown): value is ActivityNode['status'] {
  return value === 'running' || value === 'stopped';
}

function isBuildStatus(value: unknown): value is BuildNode['status'] {
  return value === 'running' || value === 'succeeded' || value === 'failed';
}
