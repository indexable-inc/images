// Shared braille spinner frame ticker.
//
// Every component that wants to show "an agent process is running"
// subscribes to `spinnerFrame` and renders its current value. There
// is exactly one timer for the whole app: subscribers all see the
// same frame, which keeps multiple active rows visually in lockstep
// (calming) instead of free-running out of sync (chaotic).
//
// The readable only ticks while it has at least one subscriber, so
// when nothing in the UI needs the spinner the timer is torn down.

import { readable, type Readable } from 'svelte/store';

// 10-frame "dots" cycle used by ora / spin.js / most modern TUIs.
// 160ms per frame reads as a calm orbital sweep — slower than the
// 80–100ms default most CLIs use, but the sidebar is ambient context,
// not foreground feedback, so unhurried fits better.
const FRAMES = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
const FRAME_MS = 160;

export const SPINNER_FRAMES: readonly string[] = FRAMES;

export const spinnerFrame: Readable<string> = readable(FRAMES[0], (set) => {
  let i = 0;
  const id = setInterval(() => {
    i = (i + 1) % FRAMES.length;
    set(FRAMES[i]);
  }, FRAME_MS);
  return () => clearInterval(id);
});
