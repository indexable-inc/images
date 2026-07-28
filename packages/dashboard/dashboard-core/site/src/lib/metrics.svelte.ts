import { measureCellRatio } from './ansi';

// Shared reactive metrics. `ratio` is the monospace cell width per font px,
// remeasured once the web font loads; `themeV` bumps on an OS light/dark flip so
// cards re-render their screen against the new palette.
export const metrics = $state({ ratio: 0.6, themeV: 0 });

export function refreshRatio(): void {
  metrics.ratio = measureCellRatio();
}

export function bumpTheme(): void {
  metrics.themeV++;
}
