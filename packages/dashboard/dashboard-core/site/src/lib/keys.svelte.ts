// The dashboard's vim keymap, owned in one place. A single capture-phase keydown
// listener drives every motion: the sidebar registers its handlers on mount and
// the global listener routes the keys to it, so there is exactly one thing
// listening to the keyboard.
//
// Capturing also lets us defend against page-level vim extensions (Vimium and
// friends): for every key we bind we stopImmediatePropagation, so the dashboard's
// own bindings win over an extension's instead of fighting it.

import { ui, clearFocus } from './ui.svelte';

// What the sidebar exposes to the global keymap. It registers these on mount and
// clears them on destroy.
export interface ListNav {
  move(delta: number): void; // j/k (±1) and Ctrl-d/Ctrl-u (±half page)
  top(): void; // gg
  bottom(): void; // G
  open(): void; // o / Enter / l — open a resource/rich-output fullscreen, or unfold
  back(): void; // h — fold the current session, or step to its header
  fold(): void; // za — toggle the fold under the selection
  filter(): void; // / — focus the sidebar filter box
}

let nav: ListNav | null = null;
export function setListNav(handlers: ListNav | null): void {
  nav = handlers;
}

// Reactive keymap state the chrome reads: whether the help overlay is open.
export const keys = $state({ help: false });

const HALF_PAGE = 8;

function isEditable(el: EventTarget | null): boolean {
  const t = el as HTMLElement | null;
  if (!t) return false;
  return (
    t.tagName === 'INPUT' || t.tagName === 'TEXTAREA' || t.tagName === 'SELECT' || t.isContentEditable
  );
}

// Install the global keymap. Returns a teardown for onMount.
export function installKeymap(): () => void {
  // Tracks a leading `g`/`z` so the next key can complete a two-key motion (gg,
  // za). Any other key clears it and is handled normally.
  let pending: '' | 'g' | 'z' = '';

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

    // Resolve a pending two-key motion.
    if (pending === 'g') {
      pending = '';
      if (e.key === 'g') {
        nav?.top();
        return swallow(e);
      }
      // not the second g — fall through and handle this key on its own
    } else if (pending === 'z') {
      pending = '';
      if (e.key === 'a') {
        nav?.fold();
        return swallow(e);
      }
      // not `za` — fall through
    }

    // Enter on a focused button or link must still activate it.
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
      case 'j':
        nav?.move(1);
        return swallow(e);
      case 'k':
        nav?.move(-1);
        return swallow(e);
      case 'g':
        pending = 'g';
        return swallow(e);
      case 'z':
        pending = 'z';
        return swallow(e);
      case 'G':
        nav?.bottom();
        return swallow(e);
      case 'l':
      case 'o':
        nav?.open();
        return swallow(e);
      case 'Enter':
        if (onControl) return; // let the focused control take it
        nav?.open();
        return swallow(e);
      case 'h':
        nav?.back();
        return swallow(e);
      case '/':
        nav?.filter();
        return swallow(e);
    }
  };

  window.addEventListener('keydown', onKey, true);
  return () => window.removeEventListener('keydown', onKey, true);
}
