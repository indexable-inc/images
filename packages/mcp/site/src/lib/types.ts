// The shape the MCP server's REST API returns. Mirrors the SQLite rows in
// ix_notebook_mcp/store.py: one execution row per python_exec run, one resource
// row per live view (a terminal screen, an HTML widget), and the agent's curated
// presentation cells.

// A rich display output captured from the kernel: an nbformat-style mime bundle.
export interface RichOutput {
  output_type?: string;
  data?: Record<string, string>;
}

// The custom mime a Result carries so the dashboard can reconstruct the exact
// model-facing view (mirrors ix_notebook_mcp.runtime.IX_LLM_MIME). Stored
// JSON-encoded in a RichOutput's `data` map; parse it to a `LlmView`.
export const IX_LLM_MIME = 'application/x-ix-llm+json';

// What the agent actually received for one output: the `llm_result` text and the
// downscaled `llm_images`, each a base64 payload with its mime.
export interface LlmView {
  text: string;
  images: { mime: string; data: string }[];
}

// 'interrupted' marks a run that was still going when its server died; set when
// the session file is reopened (store.mark_interrupted).
export type JobStatus = 'running' | 'done' | 'error' | 'cancelled' | 'interrupted';

// The live value one of a cell's identifiers was bound to when the run finished
// (ix_notebook_mcp/introspect.py). `summary` is the inlay chip shown after the
// name; `detail` and the optional `def` (a "file:line" definition site) fill the
// hover card. Keyed by identifier name in `Job.bindings`.
export interface Binding {
  kind: string;
  type: string;
  summary: string;
  detail: string;
  def?: string;
}

export interface Job {
  id: string;
  name: string;
  code: string;
  code_html: string;
  status: JobStatus;
  started_at: number;
  ended_at: number | null;
  budget: number;
  output: string;
  result: string | null;
  error: string | null;
  // The cell line currently executing (running jobs only; sampled off the
  // kernel's suspended coroutine chain) and the cell line a failure was raised
  // on. Both 1-based, matching the data-line spans in `code_html`.
  line: number | null;
  error_line: number | null;
  outputs: RichOutput[];
  bindings: Record<string, Binding>;
  // 'cell' for a normal execution; 'replay' for a re-run performed while
  // reopening a session file.
  kind: 'cell' | 'replay';
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
