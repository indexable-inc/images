// Relative-time formatter that matches the Amp screenshots.
//
// Outputs: "just now", "12s ago", "4m ago", "2h ago", "8d ago",
// "3w ago", "5mo ago", "1y ago".

const MINUTE = 60_000;
const HOUR = 60 * MINUTE;
const DAY = 24 * HOUR;
const WEEK = 7 * DAY;
const MONTH = 30 * DAY;
const YEAR = 365 * DAY;

export function relativeTime(ts: number, now: number = Date.now()): string {
  const delta = Math.max(0, now - ts);
  if (delta < 10_000) return 'just now';
  if (delta < MINUTE) return `${Math.floor(delta / 1000)}s ago`;
  if (delta < HOUR) return `${Math.floor(delta / MINUTE)}m ago`;
  if (delta < DAY) return `${Math.floor(delta / HOUR)}h ago`;
  if (delta < WEEK) return `${Math.floor(delta / DAY)}d ago`;
  if (delta < MONTH) return `${Math.floor(delta / WEEK)}w ago`;
  if (delta < YEAR) return `${Math.floor(delta / MONTH)}mo ago`;
  return `${Math.floor(delta / YEAR)}y ago`;
}

// Compact form used in the sidebar where horizontal space is tight:
// "now", "12s", "4m", "2h", "8d", "3w", "5mo", "1y".
export function relativeTimeShort(ts: number, now: number = Date.now()): string {
  const delta = Math.max(0, now - ts);
  if (delta < 10_000) return 'now';
  if (delta < MINUTE) return `${Math.floor(delta / 1000)}s`;
  if (delta < HOUR) return `${Math.floor(delta / MINUTE)}m`;
  if (delta < DAY) return `${Math.floor(delta / HOUR)}h`;
  if (delta < WEEK) return `${Math.floor(delta / DAY)}d`;
  if (delta < MONTH) return `${Math.floor(delta / WEEK)}w`;
  if (delta < YEAR) return `${Math.floor(delta / MONTH)}mo`;
  return `${Math.floor(delta / YEAR)}y`;
}

// Clock-style elapsed duration. Always shows two adjacent units so
// the smallest digit ticks every second / minute as you'd read off
// a stopwatch: "45s", "2:30", "1:23", "2d 14h". Used for the idle
// indicator, where a continuously updating "how long has Alice been
// away" reads more naturally as M:SS than as "2m ago".
export function durationClock(deltaMs: number): string {
  const sec = Math.max(0, Math.floor(deltaMs / 1000));
  if (sec < 60) return `${sec}s`;
  const min = Math.floor(sec / 60);
  if (min < 60) {
    const s = sec % 60;
    return `${min}:${s.toString().padStart(2, '0')}`;
  }
  const hr = Math.floor(min / 60);
  if (hr < 24) {
    const m = min % 60;
    return `${hr}:${m.toString().padStart(2, '0')}`;
  }
  const day = Math.floor(hr / 24);
  const h = hr % 24;
  return `${day}d ${h.toString().padStart(2, '0')}h`;
}

export function clockTime(ts: number): string {
  const d = new Date(ts);
  const hh = d.getHours().toString().padStart(2, '0');
  const mm = d.getMinutes().toString().padStart(2, '0');
  return `${hh}:${mm}`;
}

// Locale-aware short form for inline timestamps on messages.
// Same day  → "3:14 PM"
// This week → "Wed 3:14 PM"
// This year → "May 25, 3:14 PM"
// Older     → "May 25, 2025, 3:14 PM"
const WEEK_MS = 7 * 24 * 60 * 60 * 1000;
export function localTime(ts: number, now: number = Date.now()): string {
  const d = new Date(ts);
  const n = new Date(now);
  const time = d.toLocaleTimeString(undefined, { hour: 'numeric', minute: '2-digit' });
  if (d.toDateString() === n.toDateString()) return time;
  if (now - ts < WEEK_MS) {
    return `${d.toLocaleDateString(undefined, { weekday: 'short' })} ${time}`;
  }
  if (d.getFullYear() === n.getFullYear()) {
    return `${d.toLocaleDateString(undefined, { month: 'short', day: 'numeric' })}, ${time}`;
  }
  return `${d.toLocaleDateString(undefined, { month: 'short', day: 'numeric', year: 'numeric' })}, ${time}`;
}

// Spelled-out relative time for hover tooltips: "3 minutes ago",
// "2 hours ago", "yesterday". Switches to a locale date once the
// gap is more than a few days, since "5 days ago" reads worse than
// the concrete day at that range. Year is included only when the
// year differs from now.
const HUMAN_DAY_LIMIT = 4;
export function humanAgo(ts: number, now: number = Date.now()): string {
  const delta = Math.max(0, now - ts);
  if (delta < 10_000) return 'just now';
  if (delta < MINUTE) {
    const s = Math.floor(delta / 1000);
    return `${s} second${s === 1 ? '' : 's'} ago`;
  }
  if (delta < HOUR) {
    const m = Math.floor(delta / MINUTE);
    return `${m} minute${m === 1 ? '' : 's'} ago`;
  }
  if (delta < DAY) {
    const h = Math.floor(delta / HOUR);
    return `${h} hour${h === 1 ? '' : 's'} ago`;
  }
  const days = Math.floor(delta / DAY);
  if (days === 1) return 'yesterday';
  if (days <= HUMAN_DAY_LIMIT) return `${days} days ago`;
  const d = new Date(ts);
  const n = new Date(now);
  if (d.getFullYear() === n.getFullYear()) {
    return d.toLocaleDateString(undefined, { month: 'short', day: 'numeric' });
  }
  return d.toLocaleDateString(undefined, {
    month: 'short',
    day: 'numeric',
    year: 'numeric'
  });
}

// Full locale string used in hover tooltips and as the title= on
// inline timestamps so the user can always see the exact moment.
export function absoluteTime(ts: number): string {
  return new Date(ts).toLocaleString(undefined, {
    weekday: 'short',
    year: 'numeric',
    month: 'short',
    day: 'numeric',
    hour: 'numeric',
    minute: '2-digit',
    second: '2-digit'
  });
}
