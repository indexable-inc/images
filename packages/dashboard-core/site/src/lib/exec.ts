// Parsers for an exec pane's JSON-encoded text fields. The hub stores list-shaped
// fields as canonical JSON text (so they diff and replay like any body); the
// frontend parses them back. Each parser is defensive: a malformed or absent
// value yields an empty result rather than throwing, so a mixed-version dashboard
// keeps rendering.

// An exec run's rich HTML tables: one self-contained document per displayed
// DataFrame / eval result (see `_collect_html` in python_worker.py).
export function parseExecHtml(text: string | undefined): string[] {
  if (!text) return [];
  try {
    const parsed: unknown = JSON.parse(text);
    return Array.isArray(parsed) ? parsed.filter((d): d is string => typeof d === 'string') : [];
  } catch {
    return [];
  }
}
