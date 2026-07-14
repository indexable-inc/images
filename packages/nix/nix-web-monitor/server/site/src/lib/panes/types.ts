/// Wire-free data model for the pane system: a dock is a tree of splits whose
/// leaves are tab groups, plus a list of floating windows. The tree is plain
/// JSON so a layout round-trips through `localStorage` unchanged; every
/// structural rule (what a valid tree looks like) lives in `layout.svelte.ts`,
/// which also owns rehydrating an untrusted stored blob back into this shape.

import type { Snippet } from 'svelte';

export type PaneId = string;

/// What the host app registers per pane: identity, the tab label, and the
/// content to render wherever the pane currently lives (docked, collapsed
/// behind a tab, or floating). `controls` renders on the right side of the tab
/// bar while the pane is the group's active tab -- the slot for pane-specific
/// affordances that used to live in per-panel headers.
export type PaneSpec = Readonly<{
  id: PaneId;
  title: string;
  /// Conditional panes (e.g. a view only a patched backend feeds) set this
  /// false to hide their tab everywhere without forgetting their layout slot.
  visible?: boolean;
  content: Snippet;
  controls?: Snippet;
}>;

export type SplitDirection = 'row' | 'column';

/// An internal node: children laid out left-to-right (`row`) or top-to-bottom
/// (`column`), sized by `sizes` fractions that sum to ~1 (one entry per child).
export type SplitNode = {
  kind: 'split';
  direction: SplitDirection;
  sizes: number[];
  children: LayoutNode[];
};

/// A leaf: a stack of panes sharing one region, one visible at a time behind a
/// tab bar. `collapsed` shrinks the group to its tab bar alone.
export type GroupNode = {
  kind: 'group';
  tabs: PaneId[];
  active: PaneId | null;
  collapsed: boolean;
};

export type LayoutNode = SplitNode | GroupNode;

/// A pane promoted to a floating window over the dock. Coordinates are pixels
/// relative to the dock root; the array's order is the z-order (last on top).
export type FloatingPane = {
  id: PaneId;
  x: number;
  y: number;
  width: number;
  height: number;
};

export type DockLayout = {
  root: LayoutNode;
  floating: FloatingPane[];
};

/// Convenience constructors so a host's default layout reads as a tree.
export function split(
  direction: SplitDirection,
  sizes: number[],
  children: LayoutNode[]
): SplitNode {
  return { kind: 'split', direction, sizes, children };
}

export function group(tabs: PaneId[], options?: { active?: PaneId; collapsed?: boolean }): GroupNode {
  return {
    kind: 'group',
    tabs: [...tabs],
    active: options?.active ?? tabs.at(0) ?? null,
    collapsed: options?.collapsed ?? false
  };
}
