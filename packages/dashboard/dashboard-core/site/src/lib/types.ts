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
  // Optional parent pane id within the same producer, used to nest resources
  // under the run that created them.
  parent?: string;
  // Milliseconds since the epoch, stamped once when the pane first appears. Every
  // pane has it; the card renders it as a human age.
  created_at?: number;
  // The one large mutable field for terminal/html/data, interpreted by `kind`.
  body?: string;
  // terminal-only: plain scrollback above the styled viewport in `body`.
  scrollback?: string;
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
  // exec-only: wall-clock the run took, in milliseconds, set when it finishes. The
  // feed shows this instead of an age so a row reads as "how long it took".
  duration_ms?: number;
  // exec-only: fold group inside the session.
  topic?: string;
  // exec-only: 1-based source line currently executing or where an error raised.
  line?: number;
  error_line?: number;
  // Inline-trace execution: a JSON-encoded array of `{line, text}` (the hub stores
  // it as canonical text like the data body, so the frontend parses it). Each
  // entry pairs a captured stdout chunk with the 1-based source line that emitted
  // it, in order, so the feed can render output beside the line that produced it
  // and replay the run. Empty/absent for output with no line attribution (a
  // subprocess) or an older producer.
  trace?: string;
  // data-only: the name of the frontend renderer to dispatch to
  renderer?: string;
}

// A pane placed on the board: its record plus the key and the producer scope
// parsed from the key.
export interface Pane extends PaneRecord {
  key: string;
  scope: string;
}
