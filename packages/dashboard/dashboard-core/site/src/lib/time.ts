// Pure time/duration formatters, kept free of Svelte runes so they are unit
// testable and reusable. The reactive clock that drives their `refMs` argument
// lives in ui.svelte.ts.

// A compact human age ("now", "3s ago", "2m ago", "4h ago", "3d ago") for a
// `created_at` relative to a reference time.
export function humanAge(createdMs: number | undefined, refMs: number): string {
  if (!createdMs) return '';
  const seconds = Math.max(0, Math.round((refMs - createdMs) / 1000));
  if (seconds < 1) return 'now';
  if (seconds < 60) return `${seconds}s ago`;
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return `${minutes}m ago`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours}h ago`;
  return `${Math.floor(hours / 24)}d ago`;
}

// The sidebar's start-stamp: a relative age ("12s ago" / "3m ago") while under an
// hour old, then the wall-clock time ("14:32") if it started today, then a short
// date ("Jun 30") for anything older. `createdMs` is the run's start; `refMs` the
// live clock (so it re-renders as time passes).
export function humanTime(createdMs: number | undefined, refMs: number): string {
  if (!createdMs) return '';
  const seconds = Math.max(0, Math.round((refMs - createdMs) / 1000));
  if (seconds < 60) return `${Math.max(1, seconds)}s ago`;
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return `${minutes}m ago`;
  const started = new Date(createdMs);
  const now = new Date(refMs);
  const sameDay =
    started.getFullYear() === now.getFullYear() &&
    started.getMonth() === now.getMonth() &&
    started.getDate() === now.getDate();
  if (sameDay) {
    return started.toLocaleTimeString(undefined, {
      hour: '2-digit',
      minute: '2-digit',
      hour12: false,
    });
  }
  return started.toLocaleDateString(undefined, { month: 'short', day: 'numeric' });
}

// A compact run-duration label ("420ms", "1.3s", "9.8s", "1m4s").
export function humanDuration(ms: number | undefined): string {
  if (ms == null || ms < 0) return '';
  if (ms < 1000) return `${Math.round(ms)}ms`;
  const s = ms / 1000;
  if (s < 10) return `${s.toFixed(1)}s`;
  if (s < 60) return `${Math.round(s)}s`;
  const m = Math.floor(s / 60);
  return `${m}m${Math.round(s % 60)}s`;
}

// The tooltip shown on a run row: how long it took, or how long it has been
// running, so duration lives in the title rather than cluttering the row.
export function runTooltip(
  running: boolean,
  durationMs: number | undefined,
  createdMs: number | undefined,
  refMs: number,
): string {
  if (running) {
    const elapsed = createdMs ? Math.max(0, Math.round((refMs - createdMs) / 1000)) : 0;
    return `running · ${elapsed}s elapsed`;
  }
  if (durationMs != null) return `took ${humanDuration(durationMs)}`;
  return '';
}

// A wall-clock label for a timeline position.
export function humanClock(ms: number): string {
  if (!ms) return '—';
  return new Date(ms).toLocaleTimeString();
}

// A calendar-date label ("today", "yesterday", "Jun 30", "Jun 30 2025") for a
// recording's start, so the recordings list reads as dated sessions rather than
// bare ids. `refMs` is the current wall-clock (for the today/yesterday window).
export function humanDate(ms: number, refMs: number): string {
  if (!ms) return '';
  const start = new Date(ms);
  const now = new Date(refMs);
  // Count whole calendar days between the two local dates. Project each local
  // Y/M/D onto a UTC midnight so the subtraction is an exact multiple of 24h;
  // subtracting local midnights directly would be off by an hour across a DST
  // boundary (a spring-forward day is only 23h), which floors "yesterday" to
  // "today".
  const dayNumber = (d: Date) => Date.UTC(d.getFullYear(), d.getMonth(), d.getDate()) / 86_400_000;
  const days = dayNumber(now) - dayNumber(start);
  if (days === 0) return 'today';
  if (days === 1) return 'yesterday';
  const sameYear = start.getFullYear() === now.getFullYear();
  return start.toLocaleDateString(undefined, {
    month: 'short',
    day: 'numeric',
    ...(sameYear ? {} : { year: 'numeric' }),
  });
}

// The label for a saved recording: when it started and how long the session ran,
// e.g. "today 14:32 · 47m" or "Jun 30 · 2m". Reads as a dated session replay
// instead of an opaque id, so a user grasps what a recording is at a glance.
export function recordingLabel(startedMs: number, updatedMs: number, refMs: number): string {
  const clock = new Date(startedMs).toLocaleTimeString(undefined, {
    hour: '2-digit',
    minute: '2-digit',
    hour12: false,
  });
  const ran = Math.max(0, updatedMs - startedMs);
  // Sub-second "durations" are just a single snapshot; show only the start then.
  const span = ran >= 1000 ? humanDuration(ran) : '';
  const when = `${humanDate(startedMs, refMs)} ${clock}`.trim();
  return span ? `${when} · ${span}` : when;
}
