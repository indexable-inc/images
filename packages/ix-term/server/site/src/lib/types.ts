export interface SessionMeta {
  id: string;
  name: string;
  created_at_ms: number;
}

export interface Span {
  text: string;
  fg?: string;
  bg?: string;
  bold?: boolean;
  italic?: boolean;
  underline?: boolean;
  inverse?: boolean;
  dim?: boolean;
  strikethrough?: boolean;
}

export type CursorShape = 'block' | 'bar' | 'underline' | 'hollow';

export interface Cursor {
  x: number;
  y: number;
  visible: boolean;
  shape: CursorShape;
}

export interface RowUpdate {
  y: number;
  spans: Span[];
}

export interface HelloMsg {
  type: 'hello';
  conn: string;
  session: { id: string; name: string };
}

export interface GridMsg {
  type: 'grid';
  seq: number;
  cols: number;
  rows: number;
  full: boolean;
  changed: RowUpdate[];
  cursor: Cursor | null;
  app_cursor: boolean;
}

export interface DriverMsg {
  type: 'driver';
  conn: string | null;
  cols: number;
  rows: number;
}

export interface OpenMsg {
  type: 'open';
  path: string | null;
  nonce: number;
}

export interface OpenErrorMsg {
  type: 'open_error';
  message: string;
}

export interface ExitMsg {
  type: 'exit';
  code: number | null;
}

export type ServerMsg =
  | HelloMsg
  | GridMsg
  | DriverMsg
  | OpenMsg
  | OpenErrorMsg
  | ExitMsg;

export interface SessionsMsg {
  type: 'sessions';
  sessions: SessionMeta[];
}

export type ClientMsg =
  | { type: 'input'; data: string }
  | { type: 'resize'; cols: number; rows: number }
  | { type: 'refresh' }
  | { type: 'close_doc' };
