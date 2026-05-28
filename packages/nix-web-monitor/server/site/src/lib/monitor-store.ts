import * as v from 'valibot';
import { snapshotSchema, type ConnectionStatus, type MonitorSnapshot } from '$lib/types';

type SnapshotHandler = (snapshot: MonitorSnapshot) => void;
type StatusHandler = (status: ConnectionStatus) => void;

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
