import type { Point } from './types';

// Card positions on the board persist across reloads so a layout the user
// arranged stays put. Keyed by terminal key (scope<0x1f>id).
const STORAGE_KEY = 'tui-board-positions-v1';

export function loadPositions(): Record<string, Point> {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (!raw) return {};
    const parsed = JSON.parse(raw) as Record<string, unknown>;
    // Keep only well-formed {x:number, y:number} entries; a tampered or
    // partially-written value must not produce NaN card coordinates.
    const out: Record<string, Point> = {};
    for (const [key, value] of Object.entries(parsed)) {
      const p = value as Partial<Point>;
      if (p && typeof p.x === 'number' && typeof p.y === 'number') {
        out[key] = { x: p.x, y: p.y };
      }
    }
    return out;
  } catch {
    return {};
  }
}

export function savePositions(positions: Record<string, Point>): void {
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(positions));
  } catch {
    // Private-mode or quota: positions just will not persist, which is fine.
  }
}

// Initial slot for the Nth unplaced terminal: a left-to-right, top-to-bottom
// flow on a fixed grid. Terminals vary in size, so this is a starting point the
// user can drag from, not a packed layout.
const COL_W = 520;
const ROW_H = 380;
const COLS = 3;
const MARGIN = 24;

export function autoPlace(index: number): Point {
  return {
    x: MARGIN + (index % COLS) * COL_W,
    y: MARGIN + Math.floor(index / COLS) * ROW_H,
  };
}
