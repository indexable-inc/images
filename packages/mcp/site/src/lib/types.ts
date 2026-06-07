// The shape the MCP server's REST API returns. Mirrors the SQLite rows in
// ix_notebook_mcp/store.py: one execution row per python_exec run, one resource
// row per live view (a terminal screen, an HTML widget), and the agent's curated
// presentation cells.

// A rich display output captured from the kernel: an nbformat-style mime bundle.
export interface RichOutput {
  output_type?: string;
  data?: Record<string, string>;
}

export type JobStatus = 'running' | 'done' | 'error' | 'cancelled';

export interface Job {
  id: string;
  name: string;
  code: string;
  code_html: string;
  status: JobStatus;
  started_at: number;
  ended_at: number | null;
  output: string;
  result: string | null;
  error: string | null;
  outputs: RichOutput[];
}

export interface Resource {
  id: string;
  title: string;
  kind: string;
  html: string;
  status: string;
}

// One curated presentation cell: a title and the rendered outputs the agent
// chose to highlight (a table, an image, a Result's HTML). Ordered by position.
export interface Cell {
  id: string;
  title: string;
  position: number;
  outputs: RichOutput[];
  updated_at: number;
}
