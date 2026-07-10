/** Return the last timestamped item at or before `ts` from an ordered array. */
export function lastAtOrBefore<T extends { ts: number }>(marks: readonly T[], ts: number): T | undefined {
  let lo = 0;
  let hi = marks.length - 1;
  let best = -1;
  while (lo <= hi) {
    const mid = (lo + hi) >> 1;
    if (marks[mid].ts <= ts) {
      best = mid;
      lo = mid + 1;
    } else {
      hi = mid - 1;
    }
  }
  return marks[best < 0 ? 0 : best];
}
