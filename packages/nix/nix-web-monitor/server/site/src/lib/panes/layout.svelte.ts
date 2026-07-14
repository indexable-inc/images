/// Dock layout state: one reactive tree of splits and tab groups plus the
/// floating windows, with every mutation (move a tab, pop a pane out, drag a
/// splitter) funneled through methods that keep the tree normalized and
/// persisted. Persistence is plain JSON in `localStorage`; a stored blob is
/// untrusted (older schema, hand-edited, another app's key) so loading
/// revalidates the whole shape and falls back to the caller's default.

import type {
  DockLayout,
  FloatingPane,
  GroupNode,
  LayoutNode,
  PaneId,
  SplitNode
} from '$lib/panes/types';

/// Smallest fraction a split child can be dragged to, so a pane never
/// disappears behind a splitter and can always be grabbed back.
const MIN_FRACTION = 0.08;

/// Bump when the persisted shape changes incompatibly; a mismatched version
/// loads as "no stored layout" instead of a best-effort partial parse.
const STORAGE_VERSION = 1;

type StoredLayout = {
  version: number;
  root: LayoutNode;
  floating: FloatingPane[];
};

export class DockState {
  layout = $state<DockLayout>({ root: emptyGroup(), floating: [] });

  /// The pane id currently riding a tab drag, or null. Transient render state
  /// (drop-target highlighting), never persisted; lives here because HTML
  /// drag-and-drop hides `dataTransfer` payloads during `dragover`.
  dragging = $state<PaneId | null>(null);

  private readonly storageKey: string;
  private readonly defaultLayout: () => DockLayout;

  constructor(storageKey: string, defaultLayout: () => DockLayout) {
    this.storageKey = storageKey;
    this.defaultLayout = defaultLayout;
    this.layout = loadLayout(storageKey) ?? cloneLayout(defaultLayout());
  }

  /// The pane-id set of the last `reconcile`, so re-running against an
  /// unchanged registry (every host state update) is a no-op instead of a
  /// full tree rewrite.
  private reconciled = '';

  /// Drop panes the host no longer registers and slot newly registered panes
  /// into the first group, so a layout persisted by an older build never
  /// strands or loses a pane. Idempotent; the dock calls it whenever the
  /// registered pane set changes.
  reconcile(ids: readonly PaneId[]): void {
    const key = ids.join('\0');
    if (key === this.reconciled) return;
    this.reconciled = key;
    // Plain Sets on purpose: transient scratch state for one pass, never
    // observed reactively.
    // eslint-disable-next-line svelte/prefer-svelte-reactivity
    const known = new Set(ids);
    // eslint-disable-next-line svelte/prefer-svelte-reactivity
    const seen = new Set<PaneId>();
    pruneUnknown(this.layout.root, known, seen);
    this.layout.floating = this.layout.floating.filter((floating) => {
      if (!known.has(floating.id) || seen.has(floating.id)) return false;
      seen.add(floating.id);
      return true;
    });
    const missing = ids.filter((id) => !seen.has(id));
    if (missing.length > 0) {
      const target = firstGroup(this.layout.root);
      if (target === null) {
        this.layout.root = {
          kind: 'group',
          tabs: [...missing],
          active: missing[0],
          collapsed: false
        };
      } else {
        target.tabs.push(...missing);
        target.active ??= missing[0];
      }
    }
    this.normalize();
    this.persist();
  }

  activate(group: GroupNode, id: PaneId): void {
    if (!group.tabs.includes(id)) return;
    group.active = id;
    if (group.collapsed) group.collapsed = false;
    this.persist();
  }

  toggleCollapsed(group: GroupNode): void {
    group.collapsed = !group.collapsed;
    this.persist();
  }

  /// Move a tab into `target` at `index` (clamped), wherever it currently
  /// lives -- another group or a floating window. No-op when the pane is
  /// already the only tab of the target.
  moveTab(id: PaneId, target: GroupNode, index: number): void {
    const from = this.groupOf(id);
    if (from === target && target.tabs.length === 1) return;
    // Callers compute `index` against the pre-move tab list. When the tab
    // already lives in this group, detaching it first shifts everything to
    // its right one slot left, so adjust to keep "insert before the drop
    // target" true for same-group moves to the right.
    const current = from === target ? target.tabs.indexOf(id) : -1;
    const desired = current !== -1 && current < index ? index - 1 : index;
    this.detach(id);
    const at = Math.max(0, Math.min(desired, target.tabs.length));
    target.tabs.splice(at, 0, id);
    target.active = id;
    target.collapsed = false;
    this.normalize();
    this.persist();
  }

  /// Promote a pane to a floating window over the dock. `rect` seeds the
  /// window's position/size in dock-relative pixels.
  popOut(id: PaneId, rect: { x: number; y: number; width: number; height: number }): void {
    this.detach(id);
    this.layout.floating.push({ id, ...rect });
    this.normalize();
    this.persist();
  }

  /// Return a floating pane to the dock. The pane's original group may have
  /// been pruned away when it left, so it docks into the first group.
  dock(id: PaneId): void {
    const floating = this.layout.floating.find((entry) => entry.id === id);
    if (floating === undefined) return;
    this.layout.floating = this.layout.floating.filter((entry) => entry.id !== id);
    const target = firstGroup(this.layout.root);
    if (target === null) {
      this.layout.root = { kind: 'group', tabs: [id], active: id, collapsed: false };
    } else {
      target.tabs.push(id);
      target.active = id;
      target.collapsed = false;
    }
    this.normalize();
    this.persist();
  }

  isFloating(id: PaneId): boolean {
    return this.layout.floating.some((entry) => entry.id === id);
  }

  /// Raise a floating window to the top of the stack (z-order is array order).
  raise(id: PaneId): void {
    const index = this.layout.floating.findIndex((entry) => entry.id === id);
    if (index === -1 || index === this.layout.floating.length - 1) return;
    const [entry] = this.layout.floating.splice(index, 1);
    this.layout.floating.push(entry);
    this.persist();
  }

  moveFloating(id: PaneId, x: number, y: number): void {
    const entry = this.layout.floating.find((floating) => floating.id === id);
    if (entry === undefined) return;
    entry.x = x;
    entry.y = y;
  }

  resizeFloating(id: PaneId, width: number, height: number): void {
    const entry = this.layout.floating.find((floating) => floating.id === id);
    if (entry === undefined) return;
    entry.width = width;
    entry.height = height;
  }

  /// Shift the boundary between children `leftIndex` and `rightIndex` within a
  /// split by `delta` (a fraction of the split's full extent), clamped so
  /// neither vanishes. The two indices are the *visible* neighbors of the
  /// dragged splitter; hidden siblings between them keep their stored
  /// fractions untouched, so they regain their old size when they reappear.
  /// `visibleShare` is the fraction of the split the browser is actually
  /// distributing (the caller's expanded visible children); the minimum-size
  /// clamp scales by it so the floor guards the *rendered* pane size.
  resizeSplit(
    split: SplitNode,
    leftIndex: number,
    rightIndex: number,
    delta: number,
    visibleShare = 1
  ): void {
    resizeSizes(split.sizes, leftIndex, rightIndex, delta, visibleShare);
  }

  /// Find the group currently holding `id`, or null when it floats/is absent.
  groupOf(id: PaneId): GroupNode | null {
    return findGroupWith(this.layout.root, id);
  }

  /// Make a pane visible wherever it lives: activate its tab and expand its
  /// group, or raise its window. The hook for "selecting a build opens the
  /// logs" style flows in the host app.
  reveal(id: PaneId): void {
    if (this.isFloating(id)) {
      this.raise(id);
      return;
    }
    const group = this.groupOf(id);
    if (group === null) return;
    group.active = id;
    group.collapsed = false;
    this.persist();
  }

  reset(): void {
    try {
      window.localStorage.removeItem(this.storageKey);
    } catch {
      // Storage may be unavailable (private mode); the in-memory reset stands.
    }
    this.layout = cloneLayout(this.defaultLayout());
    // The default layout may predate panes registered since; let the dock's
    // reconcile pass re-slot them.
    this.reconciled = '';
  }

  persist(): void {
    const snapshot = $state.snapshot(this.layout);
    const stored: StoredLayout = {
      version: STORAGE_VERSION,
      root: snapshot.root,
      floating: snapshot.floating
    };
    try {
      window.localStorage.setItem(this.storageKey, JSON.stringify(stored));
    } catch {
      // Best-effort: a full/blocked storage only loses persistence.
    }
  }

  /// Remove `id` from whatever currently hosts it (group tab or window).
  private detach(id: PaneId): void {
    const group = this.groupOf(id);
    if (group !== null) {
      group.tabs = group.tabs.filter((tab) => tab !== id);
      if (group.active === id) group.active = group.tabs.at(0) ?? null;
    }
    this.layout.floating = this.layout.floating.filter((entry) => entry.id !== id);
  }

  /// Restore the tree invariants after a structural edit: no empty groups
  /// hanging in splits, no single-child splits, sizes matching children and
  /// summing to 1, every group's `active` among its tabs.
  private normalize(): void {
    this.layout.root = normalizeNode(this.layout.root) ?? emptyGroup();
  }
}

/// Pure core of [`DockState.resizeSplit`], exported for tests: move `delta`
/// from `sizes[rightIndex]` to `sizes[leftIndex]`, clamped so neither pane
/// renders below `MIN_FRACTION`. Sizes are fractions of the *whole* split,
/// but the browser renormalizes the visible children to fill it, so the
/// rendered size of a pane is its fraction of `visibleShare`, not of 1 --
/// with a large hidden sibling (say sizes `[0.1, 0.8, 0.1]` with the middle
/// hidden) an absolute `MIN_FRACTION` floor would pin both visible panes
/// inside the sliver `[0.08, 0.12]`. Scaling the floor by `visibleShare`
/// keeps it a floor on what the user actually sees. The floor is additionally
/// capped at the pair's midpoint so the clamp range can never invert when the
/// pair's combined share is already below two floors (a drag then only
/// equalizes the pair, never pushes a pane further down).
export function resizeSizes(
  sizes: number[],
  leftIndex: number,
  rightIndex: number,
  delta: number,
  visibleShare: number
): void {
  const left = sizes.at(leftIndex);
  const right = sizes.at(rightIndex);
  if (left === undefined || right === undefined) return;
  const floor = Math.min(MIN_FRACTION * visibleShare, (left + right) / 2);
  const applied = Math.max(floor - left, Math.min(delta, right - floor));
  sizes[leftIndex] = left + applied;
  sizes[rightIndex] = right - applied;
}

/// Whether a subtree has anything to show given the host's hidden-pane
/// predicate. A group whose every tab is hidden (or a split of such groups)
/// renders nothing and yields its space to its siblings.
export function nodeHasVisible(node: LayoutNode, isHidden: (id: PaneId) => boolean): boolean {
  if (node.kind === 'group') return node.tabs.some((tab) => !isHidden(tab));
  return node.children.some((child) => nodeHasVisible(child, isHidden));
}

function emptyGroup(): GroupNode {
  return { kind: 'group', tabs: [], active: null, collapsed: false };
}

function cloneLayout(layout: DockLayout): DockLayout {
  return JSON.parse(JSON.stringify(layout)) as DockLayout;
}

/// Depth-first first leaf group of the tree.
function firstGroup(node: LayoutNode): GroupNode | null {
  if (node.kind === 'group') return node;
  for (const child of node.children) {
    const found = firstGroup(child);
    if (found !== null) return found;
  }
  return null;
}

function findGroupWith(node: LayoutNode, id: PaneId): GroupNode | null {
  if (node.kind === 'group') return node.tabs.includes(id) ? node : null;
  for (const child of node.children) {
    const found = findGroupWith(child, id);
    if (found !== null) return found;
  }
  return null;
}

/// Drop tabs for unregistered panes and duplicate mentions of the same pane
/// (first mention wins), recording every pane seen.
function pruneUnknown(node: LayoutNode, known: ReadonlySet<PaneId>, seen: Set<PaneId>): void {
  if (node.kind === 'group') {
    node.tabs = node.tabs.filter((tab) => {
      if (!known.has(tab) || seen.has(tab)) return false;
      seen.add(tab);
      return true;
    });
    if (node.active === null || !node.tabs.includes(node.active)) {
      node.active = node.tabs.at(0) ?? null;
    }
    return;
  }
  for (const child of node.children) pruneUnknown(child, known, seen);
}

/// Returns the normalized node, or null when the subtree holds no tabs at all
/// and should be pruned by its parent.
function normalizeNode(node: LayoutNode): LayoutNode | null {
  if (node.kind === 'group') {
    if (node.active === null || !node.tabs.includes(node.active)) {
      node.active = node.tabs.at(0) ?? null;
    }
    return node.tabs.length > 0 ? node : null;
  }

  const kept: LayoutNode[] = [];
  const keptSizes: number[] = [];
  node.children.forEach((child, index) => {
    const normalized = normalizeNode(child);
    if (normalized === null) return;
    kept.push(normalized);
    keptSizes.push(node.sizes.at(index) ?? 0);
  });
  if (kept.length === 0) return null;
  if (kept.length === 1) return kept[0];

  const total = keptSizes.reduce((sum, size) => sum + Math.max(size, MIN_FRACTION), 0);
  node.children = kept;
  node.sizes = keptSizes.map((size) => Math.max(size, MIN_FRACTION) / total);
  return node;
}

/// Rehydrate a persisted layout, rejecting anything that does not match the
/// current schema exactly. Returns null (use the default) on any anomaly.
function loadLayout(storageKey: string): DockLayout | null {
  let raw: string | null;
  try {
    raw = window.localStorage.getItem(storageKey);
  } catch {
    return null;
  }
  if (raw === null) return null;
  let parsed: unknown;
  try {
    parsed = JSON.parse(raw);
  } catch {
    return null;
  }
  if (!isRecord(parsed) || parsed['version'] !== STORAGE_VERSION) return null;
  const root = parseNode(parsed['root']);
  const floating = parseFloating(parsed['floating']);
  if (root === null || floating === null) return null;
  return { root, floating };
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null;
}

function isFiniteNumber(value: unknown): value is number {
  return typeof value === 'number' && Number.isFinite(value);
}

function parseNode(value: unknown): LayoutNode | null {
  if (!isRecord(value)) return null;
  if (value['kind'] === 'group') {
    const tabs = value['tabs'];
    if (!Array.isArray(tabs) || !tabs.every((tab) => typeof tab === 'string')) return null;
    const active = value['active'];
    if (active !== null && typeof active !== 'string') return null;
    return {
      kind: 'group',
      tabs,
      active,
      collapsed: value['collapsed'] === true
    };
  }
  if (value['kind'] === 'split') {
    const direction = value['direction'];
    if (direction !== 'row' && direction !== 'column') return null;
    const sizes = value['sizes'];
    const children = value['children'];
    if (!Array.isArray(sizes) || !sizes.every((size) => typeof size === 'number')) return null;
    if (!Array.isArray(children) || children.length !== sizes.length) return null;
    const parsedChildren: LayoutNode[] = [];
    for (const child of children) {
      const parsedChild = parseNode(child);
      if (parsedChild === null) return null;
      parsedChildren.push(parsedChild);
    }
    return { kind: 'split', direction, sizes, children: parsedChildren };
  }
  return null;
}

function parseFloating(value: unknown): FloatingPane[] | null {
  if (!Array.isArray(value)) return null;
  const parsed: FloatingPane[] = [];
  for (const entry of value) {
    if (!isRecord(entry)) return null;
    const id = entry['id'];
    const x = entry['x'];
    const y = entry['y'];
    const width = entry['width'];
    const height = entry['height'];
    if (typeof id !== 'string') return null;
    if (!isFiniteNumber(x) || !isFiniteNumber(y) || !isFiniteNumber(width) || !isFiniteNumber(height)) {
      return null;
    }
    parsed.push({ id, x, y, width, height });
  }
  return parsed;
}
