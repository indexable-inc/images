// One terminal record as it arrives in the Loro doc, keyed by `scope<0x1f>id`.
export interface TermRecord {
  command?: string;
  args?: string;
  rows?: number;
  cols?: number;
  screen?: string;
  alive?: boolean;
  exit_code?: number;
  cursor_row?: number;
  cursor_col?: number;
  cursor_visible?: boolean;
  cursor_shape?: string;
}

// A terminal placed on the board: its record plus the producer scope parsed
// from the key.
export interface Term extends TermRecord {
  key: string;
  scope: string;
}

export interface Point {
  x: number;
  y: number;
}
