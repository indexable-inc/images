// Single source of truth for app-level commands.
//
// Each entry pairs:
//   - id            – string the Rust menu uses as its MenuItem id
//                     AND the slug shown in menus/palette internally
//   - eventName     – tauri event the Rust shell emits when the menu
//                     item fires; the dispatcher below listens for it
//   - label         – human-readable name (menus + palette)
//   - shortcut      – display-only key hint (palette UI)
//   - run           – the actual behaviour to execute
//
// The Rust side (`src-tauri/src/lib.rs`) defines the same `id` and
// `eventName` strings as constants; treat the strings here as the
// contract. Adding a new command means one entry here + one row in
// the Rust COMMAND_LIST constant + (optionally) a menu spot.

import { get } from 'svelte/store';
import { router } from './router';
import {
  toggleSettings,
  toggleSidebarFocus,
  togglePalette,
  closePalette,
  openNewChat,
  openIdentity
} from './ui';
import { getDraft } from './drafts';
import { mergedThreadsList } from './store';

// Action helpers. Kept here (not in menu.ts) to avoid a circular
// import with menu.ts's Tauri-runtime wrappers.
export function startNewChat() {
  closePalette();
  const route = get(router);
  if (route.name === 'thread') {
    const current = getDraft(route.threadId);
    if (current && current.server_id === route.serverId && current.text.trim() === '') return;
  }
  openNewChat();
}

// Holding ⌘] / ⌘[ auto-repeats at ~30 Hz; without this cap the renderer
// queues up dozens of thread switches and falls behind. 60ms = ~16 Hz
// still feels instant for taps but bounds the work per second.
const NAV_THROTTLE_MS = 60;
let lastNavMs = 0;

function navigateThread(delta: 1 | -1) {
  const now =
    typeof performance !== 'undefined' && performance.now ? performance.now() : Date.now();
  if (now - lastNavMs < NAV_THROTTLE_MS) return;
  lastNavMs = now;

  const list = get(mergedThreadsList);
  if (list.length === 0) return;

  const route = get(router);
  const currentId = route.name === 'thread' ? route.serverId + ':' + route.threadId : null;

  let idx: number;
  if (currentId === null) {
    // Nothing selected — go to the first thread regardless of
    // direction. Direction only matters once we're already moving
    // through the list.
    idx = 0;
  } else {
    const current = list.findIndex((t) => t.server_id + ':' + t.id === currentId);
    if (current === -1) {
      // Brand-new draft thread that hasn't synced yet. Same
      // intuition as the null case: drop into the first real chat.
      idx = 0;
    } else {
      idx = (current + delta + list.length) % list.length;
    }
  }

  const next = list[idx];
  if (next) {
    router.go(
      '/s/' + encodeURIComponent(next.server_id) + '/t/' + encodeURIComponent(next.id)
    );
  }
}

export function nextThread() {
  navigateThread(1);
}

export function previousThread() {
  navigateThread(-1);
}

// Spawn a second desktop window for multiplayer testing. The Rust
// side opens a labelled WebviewWindow with `?as=<name>` in the URL,
// which the identity layer reads to give the new window its own
// distinct presence entry.
export async function newWindow() {
  closePalette();
  if (typeof window === 'undefined' || !('__TAURI_INTERNALS__' in window)) {
    console.warn('room: newWindow is Tauri-only');
    return;
  }
  try {
    const { invoke } = await import('@tauri-apps/api/core');
    await invoke('spawn_window');
  } catch (err) {
    console.warn('room: spawn_window failed', err);
  }
}

export const COMMAND_EVENT = {
  NewChat: 'room://new-chat',
  NewWindow: 'room://new-window',
  OpenSettings: 'room://open-settings',
  ToggleSidebar: 'room://toggle-sidebar',
  TogglePalette: 'room://toggle-palette',
  NextThread: 'room://next-thread',
  PreviousThread: 'room://previous-thread',
  // Palette-only — no native menu accelerator. Reached via ⌘K →
  // "Set identity…" or via the avatar segment in the status bar.
  SetIdentity: 'room://set-identity'
} as const;

export type CommandEvent = (typeof COMMAND_EVENT)[keyof typeof COMMAND_EVENT];

export interface CommandDef {
  id: string;
  eventName: CommandEvent;
  label: string;
  /** Short symbolic shortcut, e.g. `⌘N`. Display only. */
  shortcut: string;
  /** Whether to surface this in the Cmd-K palette. */
  inPalette: boolean;
  run: () => void;
}

export const COMMANDS: readonly CommandDef[] = [
  {
    id: 'new-chat',
    eventName: COMMAND_EVENT.NewChat,
    label: 'New Chat',
    shortcut: '⌘N',
    inPalette: true,
    run: () => startNewChat()
  },
  {
    id: 'new-window',
    eventName: COMMAND_EVENT.NewWindow,
    label: 'New Window',
    shortcut: '⌘⇧N',
    inPalette: true,
    run: () => void newWindow()
  },
  {
    id: 'toggle-palette',
    eventName: COMMAND_EVENT.TogglePalette,
    label: 'Command Palette',
    shortcut: '⌘K',
    inPalette: false,
    run: () => togglePalette()
  },
  {
    id: 'toggle-sidebar',
    eventName: COMMAND_EVENT.ToggleSidebar,
    label: 'Toggle Sidebar',
    shortcut: '⌘1',
    inPalette: true,
    run: () => toggleSidebarFocus()
  },
  {
    id: 'open-settings',
    eventName: COMMAND_EVENT.OpenSettings,
    label: 'Settings',
    shortcut: '⌘,',
    inPalette: true,
    run: () => toggleSettings()
  },
  {
    id: 'next-thread',
    eventName: COMMAND_EVENT.NextThread,
    label: 'Next Chat',
    shortcut: '⌘]',
    inPalette: true,
    run: () => nextThread()
  },
  {
    id: 'previous-thread',
    eventName: COMMAND_EVENT.PreviousThread,
    label: 'Previous Chat',
    shortcut: '⌘[',
    inPalette: true,
    run: () => previousThread()
  },
  {
    id: 'set-identity',
    eventName: COMMAND_EVENT.SetIdentity,
    label: 'Set Identity…',
    shortcut: '',
    inPalette: true,
    run: () => {
      closePalette();
      openIdentity();
    }
  }
];

const HANDLERS: Map<string, () => void> = new Map(
  COMMANDS.map((c) => [c.eventName, c.run])
);

export function dispatchEvent(eventName: string): boolean {
  const fn = HANDLERS.get(eventName);
  if (!fn) return false;
  fn();
  return true;
}

export function paletteCommands(): CommandDef[] {
  return COMMANDS.filter((c) => c.inPalette);
}
