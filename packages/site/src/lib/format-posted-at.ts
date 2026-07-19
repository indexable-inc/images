// Format an ISO 8601 timestamp for display. SSR passes no zone, so the
// prerendered HTML reads identically in every visitor's zone; after
// hydration callers pass the resolved local zone.
export function formatPostedAt(postedAt: string, zone: string | undefined): string {
  const parsed = new Date(postedAt);
  const tz = zone ?? 'UTC';
  const date = new Intl.DateTimeFormat('en', {
    month: 'short',
    day: 'numeric',
    year: 'numeric',
    timeZone: tz
  }).format(parsed);
  const time = new Intl.DateTimeFormat('en', {
    hour: '2-digit',
    minute: '2-digit',
    hour12: false,
    timeZone: tz
  }).format(parsed);
  const tzNamePart = new Intl.DateTimeFormat('en', {
    timeZoneName: 'short',
    timeZone: tz
  })
    .formatToParts(parsed)
    .find((part) => part.type === 'timeZoneName');
  return `${date} · ${time} ${tzNamePart?.value ?? tz}`;
}

// Date-only variant for day-granular fields (RFC created/updated).
// Unparseable input (the template's YYYY-MM-DD placeholder) passes through.
export function formatDay(iso: string): string {
  if (Number.isNaN(Date.parse(iso))) return iso;
  return new Intl.DateTimeFormat('en', {
    month: 'short',
    day: 'numeric',
    year: 'numeric',
    timeZone: 'UTC'
  }).format(new Date(iso));
}

// '2 weeks ago' for a day-granular ISO date. Client-only: callers pass
// Date.now() after mount so prerendered HTML stays deterministic.
export function relativeDay(iso: string, now: number): string {
  const dayMs = 86_400_000;
  const days = Math.round((now - Date.parse(iso)) / dayMs);
  const rtf = new Intl.RelativeTimeFormat('en', { numeric: 'auto' });
  if (Math.abs(days) < 7) return rtf.format(-days, 'day');
  if (Math.abs(days) < 30) return rtf.format(-Math.round(days / 7), 'week');
  if (Math.abs(days) < 365) return rtf.format(-Math.round(days / 30), 'month');
  return rtf.format(-Math.round(days / 365), 'year');
}
