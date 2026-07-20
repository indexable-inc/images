import type {
  ClientMsg,
  Cursor,
  GridMsg,
  ServerMsg,
  Span
} from '$lib/types';
import { wsUrl } from '$lib/ws';

const MAX_BACKOFF_MS = 8000;

export interface OpenDoc {
  path: string;
  nonce: number;
}

export interface ExitInfo {
  code: number | null;
}

/** One terminal websocket: grid state, driver seat, doc pane, lifecycle. */
export class TermConnection {
  readonly sessionId: string;

  connected = $state(false);
  connId = $state<string | null>(null);
  cols = $state(0);
  rows = $state(0);
  lines = $state<Span[][]>([]);
  cursor = $state<Cursor | null>(null);
  appCursor = $state(false);
  seatConn = $state<string | null>(null);
  seatCols = $state(0);
  seatRows = $state(0);
  doc = $state<OpenDoc | null>(null);
  openError = $state<string | null>(null);
  exit = $state<ExitInfo | null>(null);

  private ws: WebSocket | null = null;
  private attempts = 0;
  private timer: ReturnType<typeof setTimeout> | null = null;
  private disposed = false;
  // After (re)connect, ignore incremental frames until a full grid arrives so
  // we never patch rows onto a stale or differently sized grid.
  private awaitingFull = true;

  constructor(sessionId: string) {
    this.sessionId = sessionId;
    this.connect();
  }

  get isDriver(): boolean {
    return this.connId !== null && this.seatConn === this.connId;
  }

  dispose(): void {
    this.disposed = true;
    if (this.timer !== null) {
      clearTimeout(this.timer);
    }
    this.ws?.close();
  }

  sendInput(data: string): void {
    this.send({ type: 'input', data });
  }

  sendResize(cols: number, rows: number): void {
    this.send({ type: 'resize', cols, rows });
  }

  closeDoc(): void {
    this.send({ type: 'close_doc' });
  }

  dismissError(): void {
    this.openError = null;
  }

  private send(msg: ClientMsg): void {
    if (this.ws !== null && this.ws.readyState === WebSocket.OPEN) {
      this.ws.send(JSON.stringify(msg));
    }
  }

  private connect(): void {
    if (this.disposed) {
      return;
    }
    const path = `/api/sessions/${encodeURIComponent(this.sessionId)}/ws`;
    const ws = new WebSocket(wsUrl(path));
    this.ws = ws;
    ws.onopen = () => {
      this.attempts = 0;
      this.connected = true;
      this.awaitingFull = true;
      this.send({ type: 'refresh' });
    };
    ws.onmessage = (ev: MessageEvent) => {
      const raw: unknown = ev.data;
      if (typeof raw !== 'string') {
        return;
      }
      this.handle(JSON.parse(raw) as ServerMsg);
    };
    ws.onclose = () => {
      this.connected = false;
      this.scheduleReconnect();
    };
  }

  private scheduleReconnect(): void {
    if (this.disposed) {
      return;
    }
    const delay = Math.min(500 * 2 ** this.attempts, MAX_BACKOFF_MS);
    this.attempts = Math.min(this.attempts + 1, 6);
    this.timer = setTimeout(() => {
      this.connect();
    }, delay);
  }

  private handle(msg: ServerMsg): void {
    switch (msg.type) {
      case 'hello':
        this.connId = msg.conn;
        break;
      case 'grid':
        this.applyGrid(msg);
        break;
      case 'driver':
        this.seatConn = msg.conn;
        this.seatCols = msg.cols;
        this.seatRows = msg.rows;
        break;
      case 'open':
        this.doc = msg.path === null ? null : { path: msg.path, nonce: msg.nonce };
        break;
      case 'open_error':
        this.openError = msg.message;
        break;
      case 'exit':
        this.exit = { code: msg.code };
        break;
    }
  }

  private applyGrid(msg: GridMsg): void {
    if (msg.full) {
      const next: Span[][] = Array.from({ length: msg.rows }, () => []);
      for (const row of msg.changed) {
        if (row.y >= 0 && row.y < msg.rows) {
          next[row.y] = row.spans;
        }
      }
      this.cols = msg.cols;
      this.rows = msg.rows;
      this.lines = next;
      this.awaitingFull = false;
    } else {
      if (this.awaitingFull) {
        return;
      }
      if (msg.cols !== this.cols || msg.rows !== this.rows) {
        return;
      }
      for (const row of msg.changed) {
        if (row.y >= 0 && row.y < this.lines.length) {
          this.lines[row.y] = row.spans;
        }
      }
    }
    this.cursor = msg.cursor;
    this.appCursor = msg.app_cursor;
  }
}
