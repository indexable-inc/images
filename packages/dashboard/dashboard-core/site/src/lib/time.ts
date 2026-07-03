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
