// Parsers for an exec pane's JSON-encoded text fields. The hub stores list-shaped
// fields as canonical JSON text (so they diff and replay like any body); the
// frontend parses them back. Each parser is defensive: a malformed or absent
// value yields an empty result rather than throwing, so a mixed-version dashboard
// keeps rendering.

// One rich-display output: a MIME-type -> data map (Jupyter display-data style).
// `image/*` data is base64, `text/html` a self-contained document, `text/plain`
// raw text. See `_collect_displays` in python_worker.py.
export type DisplayBundle = Record<string, string>;

// An exec run's ordered rich-display outputs: one bundle per displayed object /
// eval result / figure.
export function parseExecOutputs(text: string | undefined): DisplayBundle[] {
  if (!text) return [];
  try {
    const parsed: unknown = JSON.parse(text);
    if (!Array.isArray(parsed)) return [];
    return parsed.filter(
      (b): b is DisplayBundle => typeof b === 'object' && b !== null && !Array.isArray(b),
    );
  } catch {
    return [];
  }
}
