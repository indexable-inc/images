import * as v from 'valibot';
import type { ConnectionStatus, MonitorSnapshot } from './types';

type SnapshotHandler = (snapshot: MonitorSnapshot) => void;
type StatusHandler = (status: ConnectionStatus) => void;

const activityTypeSchema = v.object({
  code: v.number(),
  name: v.string()
});

const activityProgressSchema = v.object({
  done: v.number(),
  expected: v.number(),
  running: v.number(),
  failed: v.number()
});

const activityStatusSchema = v.picklist(['running', 'stopped']);
const buildStatusSchema = v.picklist(['running', 'stopped', 'succeeded', 'failed']);

const activityNodeSchema = v.object({
  id: v.number(),
  parent: v.nullable(v.number()),
  activityType: activityTypeSchema,
  text: v.string(),
  phase: v.nullable(v.string()),
  progress: v.nullable(activityProgressSchema),
  status: activityStatusSchema,
  startedTick: v.number(),
  stoppedTick: v.nullable(v.number()),
  build: v.nullable(v.string())
});

const buildNodeSchema = v.object({
  derivation: v.string(),
  activityId: v.nullable(v.number()),
  host: v.nullable(v.string()),
  phase: v.nullable(v.string()),
  status: buildStatusSchema,
  logCount: v.number()
});

const logEntrySchema = v.object({
  index: v.number(),
  activityId: v.nullable(v.number()),
  level: v.nullable(v.number()),
  text: v.string()
});

const snapshotSchema = v.object({
  activities: v.array(activityNodeSchema),
  builds: v.array(buildNodeSchema),
  logs: v.array(logEntrySchema),
  messages: v.array(v.string()),
  errors: v.array(v.string()),
  progress: v.nullable(activityProgressSchema),
  expected: v.record(v.string(), v.number()),
  exitCode: v.nullable(v.number()),
  finished: v.boolean()
}) satisfies v.GenericSchema<MonitorSnapshot>;

export function openMonitorEvents(onSnapshot: SnapshotHandler, onStatus: StatusHandler): () => void {
  onStatus('connecting');
  const events = new EventSource('/api/events');

  events.addEventListener('open', () => {
    onStatus('live');
  });

  events.addEventListener('snapshot', (event) => {
    const snapshot = parseSnapshotEvent(event);
    if (snapshot === null) {
      onStatus('error');
      return;
    }
    onSnapshot(snapshot);
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

function parseSnapshotEvent(event: Event): MonitorSnapshot | null {
  if (!(event instanceof MessageEvent) || typeof event.data !== 'string') return null;
  try {
    const parsed: unknown = JSON.parse(event.data);
    const result = v.safeParse(snapshotSchema, parsed);
    return result.success ? result.output : null;
  } catch {
    return null;
  }
}
