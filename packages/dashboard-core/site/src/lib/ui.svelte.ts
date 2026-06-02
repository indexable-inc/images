// Cross-component UI state: which pane is focused (the fullscreen single-resource
// view) and a once-a-second clock so every card's age stays current without each
// card holding its own timer.

export const ui = $state({
  // The key (scope<0x1f>id) of the focused pane, or null for the board.
  focusKey: null as string | null,
  // Wall-clock milliseconds, ticked every second; cards derive their age from it.
  clock: Date.now(),
});

let ticking = false;

export function startClock(): void {
  if (ticking) return;
  ticking = true;
  setInterval(() => {
    ui.clock = Date.now();
  }, 1000);
}

export function focusPane(key: string): void {
  ui.focusKey = key;
}

export function clearFocus(): void {
  ui.focusKey = null;
}

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

// A wall-clock label for a timeline position.
export function humanClock(ms: number): string {
  if (!ms) return '—';
  return new Date(ms).toLocaleTimeString();
}

// A short elapsed label ("1:23", "0:05") for the scrubber, measuring from the
// recording's start.
export function humanElapsed(ms: number): string {
  const total = Math.max(0, Math.round(ms / 1000));
  const minutes = Math.floor(total / 60);
  const seconds = total % 60;
  return `${minutes}:${seconds.toString().padStart(2, '0')}`;
}
