// Tiny shared store for UI panels that can be toggled from anywhere
// (sidebar buttons, native menu items, future keyboard shortcuts).
// Kept in its own module to avoid circular imports between Sidebar
// and the menu listener.

import { writable, get } from 'svelte/store';

export const settingsOpen = writable(false);
export const newChatOpen = writable(false);

export function toggleSettings() {
  settingsOpen.update((v) => !v);
}

export function openSettings() {
  settingsOpen.set(true);
}

export function closeSettings() {
  settingsOpen.set(false);
}

export function openNewChat() {
  newChatOpen.set(true);
}

export function closeNewChat() {
  newChatOpen.set(false);
}

export function isSettingsOpen(): boolean {
  return get(settingsOpen);
}

const SIDEBAR_STORAGE = 'room.sidebar.collapsed.v1';

function loadInitialSidebar(): boolean {
  if (typeof localStorage === 'undefined') return false;
  try {
    return localStorage.getItem(SIDEBAR_STORAGE) === '1';
  } catch {
    return false;
  }
}

export const sidebarCollapsed = writable<boolean>(loadInitialSidebar());

sidebarCollapsed.subscribe((v) => {
  if (typeof localStorage === 'undefined') return;
  try {
    localStorage.setItem(SIDEBAR_STORAGE, v ? '1' : '0');
  } catch {
    // ignore
  }
});

export function toggleSidebar() {
  sidebarCollapsed.update((v) => !v);
}

// "Active" = the sidebar owns keyboard navigation (j/k cursor, Enter
// to open). Separate from `collapsed` so we can model IntelliJ's
// project-view toggle: pressing the shortcut once focuses the panel;
// pressing again hides it; if it's hidden, it shows AND focuses.
export const sidebarActive = writable(false);

export function activateSidebar() {
  sidebarActive.set(true);
}

export function deactivateSidebar() {
  sidebarActive.set(false);
}

export function isSidebarActive(): boolean {
  return get(sidebarActive);
}

// IntelliJ-style focus toggle. Bound to ⌘1.
//   hidden            → show + focus
//   shown + focused   → hide  + drop focus
//   shown + unfocused → focus  (do not hide)
export function toggleSidebarFocus() {
  const collapsed = get(sidebarCollapsed);
  const active = get(sidebarActive);
  if (collapsed) {
    sidebarCollapsed.set(false);
    sidebarActive.set(true);
    return;
  }
  if (active) {
    sidebarCollapsed.set(true);
    sidebarActive.set(false);
    return;
  }
  sidebarActive.set(true);
}

export const paletteOpen = writable(false);

export function openPalette() {
  paletteOpen.set(true);
}

export function closePalette() {
  paletteOpen.set(false);
}

export function togglePalette() {
  paletteOpen.update((v) => !v);
}

// Identity modal — small overlay for setting display name / GitHub
// handle. Reached via the command palette and the "me" segment on
// the status bar.
export const identityOpen = writable(false);

export function openIdentity() {
  identityOpen.set(true);
}

export function closeIdentity() {
  identityOpen.set(false);
}

export function toggleIdentity() {
  identityOpen.update((v) => !v);
}

const SIDEBAR_WIDTH_STORAGE = 'room.sidebar.width.v1';
export const SIDEBAR_MIN_WIDTH = 200;
export const SIDEBAR_MAX_WIDTH = 480;
export const SIDEBAR_DEFAULT_WIDTH = 264;
// Drag below this and the sidebar snaps closed instead of clamping
// at the min width. Picked to feel like Finder / Arc behaviour.
export const SIDEBAR_COLLAPSE_THRESHOLD = 140;

function loadInitialWidth(): number {
  if (typeof localStorage === 'undefined') return SIDEBAR_DEFAULT_WIDTH;
  try {
    const raw = localStorage.getItem(SIDEBAR_WIDTH_STORAGE);
    const n = raw == null ? NaN : Number(raw);
    if (Number.isFinite(n) && n >= SIDEBAR_MIN_WIDTH && n <= SIDEBAR_MAX_WIDTH) return n;
  } catch {
    // ignore
  }
  return SIDEBAR_DEFAULT_WIDTH;
}

export const sidebarWidth = writable<number>(loadInitialWidth());

sidebarWidth.subscribe((v) => {
  if (typeof localStorage === 'undefined') return;
  try {
    localStorage.setItem(SIDEBAR_WIDTH_STORAGE, String(Math.round(v)));
  } catch {
    // ignore
  }
});

export function setSidebarWidth(px: number) {
  const clamped = Math.min(SIDEBAR_MAX_WIDTH, Math.max(SIDEBAR_MIN_WIDTH, px));
  sidebarWidth.set(clamped);
}
