// Short waiting-state copy borrowed from ix's Claude Code spinnerVerbs.
// Keep the list terse: it renders inline in the transcript while a turn runs.

const LOADING_LINES = [
  '#-ext-antithesis',
  'py mcp for ldb',
  'why not 5.5',
  '#general > anything slow?',
  'any thoughts?',
  'sf when'
] as const;

const ROTATE_MS = 2_500;

function hash32(value: string): number {
  let hash = 2166136261 >>> 0;
  for (let i = 0; i < value.length; i++) {
    hash ^= value.charCodeAt(i);
    hash = Math.imul(hash, 16777619) >>> 0;
  }
  return hash;
}

export function loadingLine(threadId: string, nowMs: number): string {
  const phase = Math.floor(nowMs / ROTATE_MS);
  const idx = (hash32(threadId) + phase) % LOADING_LINES.length;
  return LOADING_LINES[idx]!;
}
