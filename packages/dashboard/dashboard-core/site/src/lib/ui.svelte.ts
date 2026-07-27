// Cross-component UI state for the Ledger shell: the current center-stage
// selection, the sidebar's fold state, the right rail's collapse, the fullscreen
// focus overlay, and a once-a-second clock so every start-stamp stays live
// without each row holding its own timer.
import type { Selection } from './sidebar';

// Re-export the pure time formatters so components keep one import site (`ui`)
// for both the reactive clock and the labels it drives.
export {
  humanAge,
  humanTime,
  humanDuration,
  runTooltip,
  humanClock,
  humanDate,
  recordingLabel,
  shortAge,
} from './time';

// ----- fold state (persisted) --------------------------------------------
// One boolean per fold key: a section id ('sessions'/'resources'/'recordings')
// or a session scope prefixed 'sess:'. Persisted so the tree stays how you left
// it. Sections default open; individual sessions default open too, so a fresh
// load shows the newest session's runs without a click.
const FOLD_KEY = 'dash-folds-v1';

function loadFolds(): Record<string, boolean> {
  try {
    const raw = localStorage.getItem(FOLD_KEY);
    const parsed = raw ? JSON.parse(raw) : null;
    return parsed && typeof parsed === 'object' ? (parsed as Record<string, boolean>) : {};
  } catch {
    return {};
  }
}

// ----- persisted boolean panes -------------------------------------------
// The rail's collapse and the history panel's visibility are the same kind of
// state: one boolean each, mirrored to localStorage under its own key, and off
// when storage is unavailable or has never been written.
const RAIL_KEY = 'dash-rail-collapsed-v1';
// The history is an inspection surface, not the reading surface, so it is
// closed by default: an unasked-for column on every load is chrome.
const HISTORY_KEY = 'dash-history-open-v1';

function loadFlag(key: string): boolean {
  try {
    return localStorage.getItem(key) === '1';
  } catch {
    return false;
  }
}

// Flip a persisted flag and mirror it back. A storage failure (private mode,
// quota) is not worth failing the toggle over: the flag still flips, it just
// resets to its default next load.
function toggleFlag(field: 'railCollapsed' | 'historyOpen', key: string): void {
  ui[field] = !ui[field];
  try {
    localStorage.setItem(key, ui[field] ? '1' : '0');
  } catch {
    // Non-persistent is fine.
  }
}

export const ui = $state({
  // The center-stage target: a run, a resource, or a recording (null = none yet).
  selection: null as Selection | null,
  // The key (scope<0x1f>id) of the focused pane in the fullscreen single-pane
  // overlay (opened with o/Enter on a resource or rich output), or null.
  focusKey: null as string | null,
  // Fold state by key; missing keys fall back to the per-key default (see isOpen).
  folds: loadFolds() as Record<string, boolean>,
  // Whether the right-rail namespace inspector is collapsed.
  railCollapsed: loadFlag(RAIL_KEY),
  // Whether the edit-history panel is showing.
  historyOpen: loadFlag(HISTORY_KEY),
  // Wall-clock milliseconds, ticked every second; rows derive their age from it.
  clock: Date.now(),
});

// A fold key's open state, with the default applied for keys never toggled. Every
// section and session defaults open.
export function isOpen(foldKey: string): boolean {
  return ui.folds[foldKey] ?? true;
}

export function toggleFold(foldKey: string): void {
  ui.folds[foldKey] = !isOpen(foldKey);
  persistFolds();
}

export function setFold(foldKey: string, open: boolean): void {
  ui.folds[foldKey] = open;
  persistFolds();
}

function persistFolds(): void {
  try {
    localStorage.setItem(FOLD_KEY, JSON.stringify(ui.folds));
  } catch {
    // Non-persistent is fine; folds reset to defaults next load.
  }
}

export function select(selection: Selection | null): void {
  ui.selection = selection;
}

export function toggleRail(): void {
  toggleFlag('railCollapsed', RAIL_KEY);
}

let ticking = false;

export function startClock(): void {
  if (ticking) return;
  ticking = true;
  setInterval(() => {
    ui.clock = Date.now();
  }, 1000);
}

export function toggleHistory(): void {
  toggleFlag('historyOpen', HISTORY_KEY);
}

export function focusPane(key: string): void {
  ui.focusKey = key;
}

export function clearFocus(): void {
  ui.focusKey = null;
}

