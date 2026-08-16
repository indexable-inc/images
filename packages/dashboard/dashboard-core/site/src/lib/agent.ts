// The agent view's data layer: parsing the transcript pane a tui producer
// publishes beside every agent terminal, and the `inputs` keys the compose box
// writes. Kept free of DOM imports so it is unit testable in plain node.
//
// The wire (packages/tui/tui/src/frame + transcript): an agent terminal pane
// carries `agent` (kind label) and `status`; its companion `data` pane
// (renderer:'transcript', parent = the terminal's pane id) holds a JSON body
// `{agent, entries, skipped}` whose entries only append within the tail's
// window, so rows can be keyed by index and scroll position survives a frame.
import { SCOPE_SEP, paneScope } from './scope.ts';
import { isTranscriptPane, paneId } from './run.ts';
import type { PaneRecord } from './types.ts';

// One transcript row: who said it, what they said, the tool they called, and
// what it cost. Mirrors tui::transcript::TranscriptEntry.
export interface TranscriptRow {
  role: string;
  text: string;
  tool?: string;
  usage?: { input_tokens: number; output_tokens: number };
  ts?: string;
}

export interface Transcript {
  entries: TranscriptRow[];
  // Session-log lines the producer could not parse. A climbing count is the
  // format-drift alarm, so the view surfaces it instead of hiding it.
  skipped: number;
}

function asUsage(raw: unknown): TranscriptRow['usage'] {
  if (!raw || typeof raw !== 'object') return undefined;
  const u = raw as { input_tokens?: unknown; output_tokens?: unknown };
  if (typeof u.input_tokens !== 'number' || typeof u.output_tokens !== 'number') return undefined;
  return { input_tokens: u.input_tokens, output_tokens: u.output_tokens };
}

// Parse a transcript pane's body. Anything malformed degrades to an empty
// transcript rather than throwing: the pane is producer-written, and a bad
// frame must not take the agent card down with it.
export function parseTranscript(body: string | undefined): Transcript {
  let parsed: unknown = null;
  try {
    parsed = JSON.parse(body ?? '');
  } catch {
    return { entries: [], skipped: 0 };
  }
  if (!parsed || typeof parsed !== 'object') return { entries: [], skipped: 0 };
  const raw = parsed as { entries?: unknown; skipped?: unknown };
  const entries: TranscriptRow[] = [];
  if (Array.isArray(raw.entries)) {
    for (const entry of raw.entries) {
      if (!entry || typeof entry !== 'object') continue;
      const e = entry as { role?: unknown; text?: unknown; tool?: unknown; usage?: unknown; ts?: unknown };
      if (typeof e.role !== 'string') continue;
      entries.push({
        role: e.role,
        text: typeof e.text === 'string' ? e.text : '',
        tool: typeof e.tool === 'string' ? e.tool : undefined,
        usage: asUsage(e.usage),
        ts: typeof e.ts === 'string' ? e.ts : undefined,
      });
    }
  }
  return { entries, skipped: typeof raw.skipped === 'number' ? raw.skipped : 0 };
}

// The key one viewer input lives under in the `inputs` root, matching
// dashboard::hub::input_key.
export function inputKey(scope: string, pane: string, field: string): string {
  return `${scope}${SCOPE_SEP}${pane}${SCOPE_SEP}${field}`;
}

// The companion transcript pane's key for an agent terminal, or null while the
// producer has not published one (an agent whose session log has not appeared
// yet publishes only the terminal pane).
export function transcriptKeyFor(panes: Record<string, PaneRecord>, termKey: string): string | null {
  const scope = paneScope(termKey);
  const id = paneId(termKey);
  for (const [key, pane] of Object.entries(panes)) {
    if (paneScope(key) === scope && isTranscriptPane(pane) && pane.parent === id) return key;
  }
  return null;
}
