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
