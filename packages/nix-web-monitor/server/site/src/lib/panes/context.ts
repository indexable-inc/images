/// Dock-scoped context shared by the pane components, so deeply nested groups
/// and windows reach the one `DockState` and the host's pane registry without
/// prop-drilling through every split level.

import { getContext, setContext } from 'svelte';
import type { DockState } from '$lib/panes/layout.svelte';
import type { PaneId, PaneSpec } from '$lib/panes/types';

export type DockContext = Readonly<{
  state: DockState;
  /// The registered spec for a pane id; undefined for a stale id (the state
  /// layer prunes those, so render code treats it as "skip").
  spec: (id: PaneId) => PaneSpec | undefined;
  /// Host-driven conditional visibility (spec.visible === false).
  hidden: (id: PaneId) => boolean;
  /// The dock's root element, for dock-relative window geometry.
  dockElement: () => HTMLElement | null;
}>;

const KEY = Symbol('pane-dock');

export function setDockContext(context: DockContext): void {
  setContext(KEY, context);
}

export function getDockContext(): DockContext {
  return getContext<DockContext>(KEY);
}

/// Per-pane visibility, threaded to the pane's *content* subtree. Panes stay
/// mounted while hidden (inactive tab, collapsed group), so any global
/// listeners they own -- e.g. a `<svelte:window onkeydown>` -- keep firing.
/// Content that grabs window-level input must check this and stand down while
/// its pane is hidden, or a background tab steals keystrokes from the visible
/// one. A function (not a boolean) so the value stays live across reads.
export type PaneVisibility = () => boolean;

const VISIBILITY_KEY = Symbol('pane-visibility');

export function setPaneVisibility(visible: PaneVisibility): void {
  setContext(VISIBILITY_KEY, visible);
}

/// Defaults to "visible" so pane content rendered outside the dock (tests, a
/// host that doesn't use panes) keeps its shortcuts.
export function getPaneVisibility(): PaneVisibility {
  return getContext<PaneVisibility | undefined>(VISIBILITY_KEY) ?? (() => true);
}
