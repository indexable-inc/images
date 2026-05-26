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
