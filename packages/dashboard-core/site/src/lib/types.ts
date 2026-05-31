// One pane record as it arrives in the Loro doc, keyed by `scope<0x1f>id`.
//
// The hub stores common scalars (`kind`, `title`, `subtitle`) plus one `body`
// text — a terminal screen, an HTML document, or a data view's JSON — and, for
// terminals, the geometry and cursor/exit scalars. A renderer reads only the
// fields its kind defines; the rest stay undefined.
export interface PaneRecord {
  kind?: string;
  title?: string;
  subtitle?: string;
  // The one large mutable field, interpreted by `kind`.
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
