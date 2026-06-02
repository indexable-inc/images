// One pane record as it arrives in the Loro doc, keyed by `scope<0x1f>id`.
//
// The hub stores common scalars (`kind`, `created_at`, `title`, `subtitle`) plus
// one Loro text per large field the view declares — a terminal/html/data `body`,
// or an exec's `source`/`stdout`/`stderr`/`result` — and the view's own scalars.
// A renderer reads only the fields its kind defines; the rest stay undefined.
export interface PaneRecord {
  kind?: string;
  title?: string;
  subtitle?: string;
  // Milliseconds since the epoch, stamped once when the pane first appears. Every
  // pane has it; the card renders it as a human age.
  created_at?: number;
  // The one large mutable field for terminal/html/data, interpreted by `kind`.
  body?: string;
  // terminal-only geometry, cursor, and exit state
  rows?: number;
  cols?: number;
  alive?: boolean;
  exit_code?: number;
  cursor_row?: number;
  cursor_col?: number;
  cursor_visible?: boolean;
  cursor_shape?: string;
  // exec-only: the source behind the run, its captured streams, and its status
  source?: string;
  stdout?: string;
  stderr?: string;
  result?: string;
  running?: boolean;
  ok?: boolean;
  lang?: string;
  // data-only: the name of the frontend renderer to dispatch to
  renderer?: string;
}

// A pane placed on the board: its record plus the key and the producer scope
// parsed from the key.
export interface Pane extends PaneRecord {
  key: string;
  scope: string;
}

export interface Point {
  x: number;
  y: number;
}
