// Tauri runtime helpers: dispatcher for native menu events into the
// JS command map, plus thin invoke wrappers for AppKit features
// (traffic lights, haptics). The @tauri-apps/api imports are loaded
// lazily so the browser dev flow (npm run dev, no Tauri runtime)
// stays no-op-safe.

import { COMMAND_EVENT, dispatchEvent, startNewChat } from './commands';

// Re-export so existing imports (Sidebar.svelte, etc.) keep working
// without churn while the action lives in commands.ts.
export { startNewChat };

function isTauriRuntime(): boolean {
  return typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;
}

export async function setTrafficLightsVisible(visible: boolean): Promise<void> {
  if (!isTauriRuntime()) return;
  try {
    const { invoke } = await import('@tauri-apps/api/core');
    await invoke('set_traffic_lights', { visible });
  } catch (err) {
    console.warn('room: set_traffic_lights failed', err);
  }
}

export type HapticKind = 'alignment' | 'level' | 'generic';

// macOS Force Touch buzz. No-op on hardware without it, no-op outside Tauri.
export async function hapticFeedback(kind: HapticKind = 'alignment'): Promise<void> {
  if (!isTauriRuntime()) return;
  try {
    const { invoke } = await import('@tauri-apps/api/core');
    await invoke('haptic_feedback', { kind });
  } catch (err) {
    console.warn('room: haptic_feedback failed', err);
  }
}

// Tauri menu → JS event bridge. The Rust shell emits one of the
// `room://...` events from src/lib/commands.ts for every menu click
// or accelerator press; dispatch through the central handler map so
// all wiring lives in one place.
export async function bindMenu(): Promise<() => void> {
  if (!isTauriRuntime()) return () => {};

  const { listen } = await import('@tauri-apps/api/event');
  const eventNames = Object.values(COMMAND_EVENT);
  const unlisteners = await Promise.all(
    eventNames.map((name) =>
      listen(name, () => {
        if (!dispatchEvent(name)) {
          console.warn('room: no handler for', name);
        }
      })
    )
  );

  return () => unlisteners.forEach((u) => u());
}
