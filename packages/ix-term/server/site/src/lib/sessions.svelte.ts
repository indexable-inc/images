import { listSessions } from '$lib/api';
import type { SessionsMsg } from '$lib/types';
import type { SessionMeta } from '$lib/types';
import { wsUrl } from '$lib/ws';

const MAX_BACKOFF_MS = 8000;

/** Live session list, fed by /api/ws with a REST fallback while it is down. */
export class SessionsStore {
  sessions = $state<SessionMeta[]>([]);

  private ws: WebSocket | null = null;
  private attempts = 0;
  private timer: ReturnType<typeof setTimeout> | null = null;
  private disposed = false;

  start(): void {
    // Seed from REST so the tab bar is useful before the socket settles.
    void this.refreshFromRest();
    this.connect();
  }

  dispose(): void {
    this.disposed = true;
    if (this.timer !== null) {
      clearTimeout(this.timer);
    }
    this.ws?.close();
  }

  private async refreshFromRest(): Promise<void> {
    try {
      this.sessions = await listSessions();
    } catch {
      // Server unreachable; the reconnect loop will retry.
    }
  }

  private connect(): void {
    if (this.disposed) {
      return;
    }
    const ws = new WebSocket(wsUrl('/api/ws'));
    this.ws = ws;
    ws.onopen = () => {
      this.attempts = 0;
    };
    ws.onmessage = (ev: MessageEvent) => {
      const raw: unknown = ev.data;
      if (typeof raw !== 'string') {
        return;
      }
      const msg = JSON.parse(raw) as SessionsMsg;
      this.sessions = msg.sessions;
    };
    ws.onclose = () => {
      this.scheduleReconnect();
    };
  }

  private scheduleReconnect(): void {
    if (this.disposed) {
      return;
    }
    void this.refreshFromRest();
    const delay = Math.min(500 * 2 ** this.attempts, MAX_BACKOFF_MS);
    this.attempts = Math.min(this.attempts + 1, 6);
    this.timer = setTimeout(() => {
      this.connect();
    }, delay);
  }
}
