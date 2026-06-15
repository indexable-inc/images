// The dashboard's vim keymap, owned in one place. A single capture-phase keydown
// listener drives every motion: top-level navigation (switch views, open the help
// overlay) lives here; each list surface registers a tiny handler for its in-view
// motions (j/k/o/h…) so the same keys work no matter which view is active and
// there is exactly one thing listening to the keyboard.
//
// Capturing also lets us defend against page-level vim extensions (Vimium and
// friends): for every key we bind we stopImmediatePropagation, so the dashboard's
// own bindings win over an extension's instead of fighting it. A browser extension
// can't be uninstalled from the page, so the guaranteed off-switch is still adding
// this origin to the extension's excluded URLs — but for the keys we own this keeps
// the two from double-firing.

import { ui, setView, clearFocus, type View } from './ui.svelte';

// What a focusable list view exposes to the global keymap. A view registers these
// on mount and clears them on destroy; the global handler routes the motion keys
// to whichever view is currently mounted.
export interface ListNav {
  move(delta: number): void; // j/k (±1) and Ctrl-d/Ctrl-u (±half page)
  top(): void; // gg
  bottom(): void; // G
  open(): void; // o / Enter / l — open or expand the selection
  back?(): void; // h — collapse, or step out to the parent (trees only)
  filter?(): void; // / — focus this view's filter box, if it has one
}

let nav: ListNav | null = null;
export function setListNav(handlers: ListNav | null): void {
  nav = handlers;
}

// Reactive keymap state the chrome reads: whether the help overlay is open.
export const keys = $state({ help: false });

// View order for [ / ] cycling, matching the rail's top-to-bottom order.
const VIEW_ORDER: readonly View[] = ['feed', 'namespace', 'board'];
const HALF_PAGE = 8;

function isEditable(el: EventTarget | null): boolean {
  const t = el as HTMLElement | null;
  if (!t) return false;
  return (
    t.tagName === 'INPUT' ||
    t.tagName === 'TEXTAREA' ||
    t.tagName === 'SELECT' ||
    t.isContentEditable
  );
}

function cycleView(dir: 1 | -1): void {
  const i = VIEW_ORDER.indexOf(ui.view);
  setView(VIEW_ORDER[(i + dir + VIEW_ORDER.length) % VIEW_ORDER.length]);
}

// Install the global keymap. Returns a teardown for onMount.
export function installKeymap(): () => void {
  // Tracks a leading `g` so the next `g` means "go to top" (gg). Any other key
  // clears it and is handled normally, so a stray `g` is harmless.
  let pendingG = false;

  const swallow = (e: KeyboardEvent): void => {
    e.preventDefault();
    e.stopImmediatePropagation();
  };

  const onKey = (e: KeyboardEvent): void => {
    // While typing in a field, stay out of the way — except Escape, which blurs so
    // keyboard navigation resumes.
    if (isEditable(e.target)) {
      if (e.key === 'Escape') (e.target as HTMLElement).blur();
      return;
    }

    // The help overlay is mostly modal: Esc / ? / q close it; nothing else leaks.
    if (keys.help) {
      if (e.key === 'Escape' || e.key === '?' || e.key === 'q') {
        keys.help = false;
        swallow(e);
      }
      return;
    }

    // Leave OS and app chords (⌘/Alt) alone. Ctrl is handled below for d/u only.
    if (e.metaKey || e.altKey) return;

    if (e.ctrlKey) {
      if (e.key === 'd') {
        nav?.move(HALF_PAGE);
        swallow(e);
      } else if (e.key === 'u') {
        nav?.move(-HALF_PAGE);
        swallow(e);
      }
      return;
    }

    // Resolve a pending `gg`.
    if (pendingG) {
      pendingG = false;
      if (e.key === 'g') {
        nav?.top();
        swallow(e);
        return;
      }
      // not the second g — fall through and handle this key on its own
    }

    // Enter / Space on a focused button or link must still activate it.
    const ae = document.activeElement;
    const onControl = !!ae && (ae.tagName === 'BUTTON' || ae.tagName === 'A');

    switch (e.key) {
      case '?':
        keys.help = true;
        return swallow(e);
      case 'Escape':
        if (ui.focusKey) {
          clearFocus();
          return swallow(e);
        }
        return;
      case '1':
        setView('feed');
        return swallow(e);
      case '2':
        setView('namespace');
        return swallow(e);
      case '3':
        setView('board');
        return swallow(e);
      case '[':
        cycleView(-1);
        return swallow(e);
      case ']':
        cycleView(1);
        return swallow(e);
      case 'j':
        nav?.move(1);
        return swallow(e);
      case 'k':
        nav?.move(-1);
        return swallow(e);
      case 'g':
        pendingG = true;
        return swallow(e);
      case 'G':
        nav?.bottom();
        return swallow(e);
      case 'l':
        nav?.open();
        return swallow(e);
      case 'o':
        nav?.open();
        return swallow(e);
      case 'Enter':
        if (onControl) return; // let the focused control take it
        nav?.open();
        return swallow(e);
      case 'h':
        if (nav?.back) {
          nav.back();
          return swallow(e);
        }
        return;
      case '/':
        if (nav?.filter) {
          nav.filter();
          return swallow(e);
        }
        return;
    }
  };

  window.addEventListener('keydown', onKey, true);
  return () => window.removeEventListener('keydown', onKey, true);
}
