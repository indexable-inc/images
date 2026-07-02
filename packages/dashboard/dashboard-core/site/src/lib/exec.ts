// Parsers for an exec pane's captured streams: its inline trace and its Python
// traceback. Split from run.ts because these touch ansi (DOM-adjacent), keeping
// run.ts's classification importable in plain node for tests.
import { stripAnsi } from './ansi.ts';

// The hub stores an exec's inline-trace as JSON text; parse it back to the
// `{line, text}[]` the trace view consumes. Malformed/absent → no trace.
export function parseTrace(raw: string | undefined): { line: number; text: string }[] {
  if (!raw) return [];
  try {
    const value = JSON.parse(raw);
    return Array.isArray(value) ? value : [];
  } catch {
    return [];
  }
}

export type ErrInfo = { message: string; frames: { line: number; text: string }[] };

// Pull the essentials out of a Python traceback so a failed run shows *where* it
// broke: the final `Type: message` line, plus each frame in the user's own exec
// source (filename `<ix-mcp exec>`) mapped to that source line. A transport error
// (no traceback) yields just its message, no frames.
export function parseError(stderr: string | undefined, source: string | undefined): ErrInfo | null {
  const text = stripAnsi(stderr ?? '').trimEnd();
  if (!text) return null;
  const lines = text.split('\n');
  const message = lines.filter((l) => l.trim()).at(-1)?.trim() ?? text;
  const src = (source ?? '').split('\n');
  const frames: { line: number; text: string }[] = [];
  for (const m of text.matchAll(/File "([^"]*)", line (\d+)/g)) {
    const file = m[1];
    const line = Number(m[2]);
    if (file.includes('ix-mcp') || file.startsWith('<')) {
      frames.push({ line, text: (src[line - 1] ?? '').trim() });
    }
  }
  return { message, frames };
}
