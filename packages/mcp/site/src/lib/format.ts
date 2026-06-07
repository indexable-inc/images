// Format a duration as a compact, minimal string. Sub-second runs read as no
// time at all (the dominant case, and noise on a finished card); otherwise a
// single whole unit, no decimals.
export function duration(seconds: number): string {
  if (seconds < 1) return '';
  if (seconds < 60) return `${Math.round(seconds)}s`;
  const m = Math.floor(seconds / 60);
  const s = Math.floor(seconds % 60);
  if (m < 60) return `${m}m ${s}s`;
  const h = Math.floor(m / 60);
  return `${h}h ${m % 60}m`;
}

// A card title for a run: only a caller-supplied `name`. With the source shown
// highlighted on the card, echoing its first line as a title read as noise, so
// an unnamed run stays untitled.
export function jobTitle(name: string, id: string): string {
  return name && name !== id ? name : '';
}
