import type { Span } from '$lib/types';

export const DEFAULT_FG = '#ddd';
export const DEFAULT_BG = '#111';

export function spanStyle(span: Span): string {
  let fg = span.fg ?? DEFAULT_FG;
  let bg = span.bg ?? DEFAULT_BG;
  if (span.inverse) {
    const swap = fg;
    fg = bg;
    bg = swap;
  }
  const parts = [`color:${fg}`];
  // Skip painting the default background so the page background shows through.
  if (bg !== DEFAULT_BG || span.inverse) {
    parts.push(`background-color:${bg}`);
  }
  if (span.bold) {
    parts.push('font-weight:bold');
  }
  if (span.italic) {
    parts.push('font-style:italic');
  }
  const deco: string[] = [];
  if (span.underline) {
    deco.push('underline');
  }
  if (span.strikethrough) {
    deco.push('line-through');
  }
  if (deco.length > 0) {
    parts.push(`text-decoration-line:${deco.join(' ')}`);
  }
  if (span.dim) {
    parts.push('opacity:0.6');
  }
  return parts.join(';');
}
