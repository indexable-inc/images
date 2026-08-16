/**
 * Line diff between two version sources, via longest common subsequence.
 * Versions are small documents, so the O(n*m) table is fine and keeps the
 * template dependency-free.
 */

export type DiffLine =
  | { kind: 'same'; text: string; aLine: number; bLine: number }
  | { kind: 'del'; text: string; aLine: number }
  | { kind: 'add'; text: string; bLine: number };

export function diffLines(a: string, b: string): DiffLine[] {
  const aLines = a.split('\n');
  const bLines = b.split('\n');
  const n = aLines.length;
  const m = bLines.length;

  // lcs[i][j] = length of the LCS of aLines[i..] and bLines[j..].
  const lcs: Uint32Array[] = Array.from(
    { length: n + 1 },
    () => new Uint32Array(m + 1)
  );
  for (let i = n - 1; i >= 0; i--) {
    for (let j = m - 1; j >= 0; j--) {
      lcs[i][j] =
        aLines[i] === bLines[j]
          ? lcs[i + 1][j + 1] + 1
          : Math.max(lcs[i + 1][j], lcs[i][j + 1]);
    }
  }

  const out: DiffLine[] = [];
  let i = 0;
  let j = 0;
  while (i < n && j < m) {
    if (aLines[i] === bLines[j]) {
      out.push({ kind: 'same', text: aLines[i], aLine: i + 1, bLine: j + 1 });
      i++;
      j++;
    } else if (lcs[i + 1][j] >= lcs[i][j + 1]) {
      out.push({ kind: 'del', text: aLines[i], aLine: i + 1 });
      i++;
    } else {
      out.push({ kind: 'add', text: bLines[j], bLine: j + 1 });
      j++;
    }
  }
  for (; i < n; i++) out.push({ kind: 'del', text: aLines[i], aLine: i + 1 });
  for (; j < m; j++) out.push({ kind: 'add', text: bLines[j], bLine: j + 1 });
  return out;
}

export type DiffHunk =
  | { kind: 'lines'; lines: DiffLine[] }
  | { kind: 'skip'; count: number };

/**
 * Fold long unchanged runs so the diff view shows changes with `context`
 * lines around them instead of the whole document.
 */
export function foldContext(lines: DiffLine[], context = 3): DiffHunk[] {
  const keep = new Array<boolean>(lines.length).fill(false);
  lines.forEach((line, index) => {
    if (line.kind === 'same') return;
    const from = Math.max(0, index - context);
    const to = Math.min(lines.length - 1, index + context);
    for (let k = from; k <= to; k++) keep[k] = true;
  });

  const hunks: DiffHunk[] = [];
  let index = 0;
  while (index < lines.length) {
    const start = index;
    while (index < lines.length && keep[index] === keep[start]) index++;
    const run = lines.slice(start, index);
    // Folding a run shorter than what the marker row costs helps nobody.
    if (!keep[start] && run.length > 2) {
      hunks.push({ kind: 'skip', count: run.length });
    } else {
      const last = hunks[hunks.length - 1];
      if (last?.kind === 'lines') last.lines.push(...run);
      else hunks.push({ kind: 'lines', lines: run });
    }
  }
  return hunks;
}
