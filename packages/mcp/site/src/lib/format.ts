// Format a duration in seconds as a compact human string.
export function duration(seconds: number): string {
  if (seconds < 60) return `${seconds.toFixed(1)}s`;
  const m = Math.floor(seconds / 60);
  const s = Math.floor(seconds % 60);
  if (m < 60) return `${m}m ${s}s`;
  const h = Math.floor(m / 60);
  return `${h}h ${m % 60}m`;
}

// A readable card title for a run. A caller-supplied `name` wins; otherwise the
// first non-empty line of the source reads as the label (an agent's leading
// `# intent` comment, or the first statement) — never the opaque job hash.
export function jobTitle(name: string, id: string, code: string): string {
  if (name && name !== id) return name;
  return codeTitle(code);
}

function codeTitle(code: string): string {
  const line = code
    .split('\n')
    .map((l) => l.trim())
    .find((l) => l.length > 0);
  if (!line) return 'python';
  return line.length > 72 ? line.slice(0, 72) + '\u2026' : line;
}
