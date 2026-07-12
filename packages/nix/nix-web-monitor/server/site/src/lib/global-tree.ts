/// Turns one coordinator's goal list (from `nix store builds --graph --json`)
/// into a renderable dependency forest. Each goal's `waiters` names the goals
/// that *want* it (edges point upward), so inverting them yields parent ->
/// children, and the coordinator's `roots` are the goals the client asked for.
///
/// Mirrors `build-tree.ts` in spirit but stays a forest: a coordinator can
/// request several roots, and unlike the invocation tree there is no synthetic
/// command root to hang them under (the coordinator header plays that role).
///
/// A goal wanted by several parents (a diamond) appears under each of them,
/// but only its first appearance expands its subtree; later appearances render
/// as leaf references. That bounds the walk on any document, including a
/// cyclic one a healthy nix never writes.

import type { GlobalCoordinator, GlobalGoal, GlobalGoalStatus } from '$lib/types';

export type GoalForest = Readonly<{
  goalById: ReadonlyMap<string, GlobalGoal>;
  /// Direct dependencies per goal id, in display order (running work first).
  childrenById: ReadonlyMap<string, readonly string[]>;
  /// The forest's top-level goal ids: the coordinator's requested roots, plus
  /// any goal nothing waits on (so a drifted or missing roots list still
  /// renders every goal somewhere).
  roots: readonly string[];
  /// Goal count per status, for the coordinator header.
  counts: Readonly<Record<GlobalGoalStatus, number>>;
}>;

/// One visible row of the flattened forest, in render order.
export type GoalRow = Readonly<{
  goal: GlobalGoal;
  depth: number;
  /// Whether the row has children (an expand/collapse toggle earns its spot).
  hasChildren: boolean;
  /// A repeat appearance of a goal already expanded above (a diamond); the row
  /// renders without descendants and without a toggle.
  repeat: boolean;
}>;

/// Display order within one parent: live work first, then queued, then the
/// session's completed record; name for a stable tie-break.
const STATUS_ORDER: Record<GlobalGoalStatus, number> = {
  running: 0,
  waiting: 1,
  other: 2,
  done: 3,
  failed: 3
};

export function buildGoalForest(coordinator: GlobalCoordinator): GoalForest {
  const goalById = new Map<string, GlobalGoal>();
  const counts: Record<GlobalGoalStatus, number> = {
    waiting: 0,
    running: 0,
    done: 0,
    failed: 0,
    other: 0
  };
  for (const goal of coordinator.goals) {
    // An id-less goal (a drifted document) cannot join the forest.
    if (goal.id.length === 0) continue;
    goalById.set(goal.id, goal);
    counts[goal.status] += 1;
  }

  const children = new Map<string, string[]>();
  const hasParent = new Set<string>();
  for (const goal of goalById.values()) {
    for (const waiter of goal.waiters) {
      // Ignore edges into goals the document does not list; the writer only
      // emits known ids, but this keeps the view honest if that ever drifts.
      if (!goalById.has(waiter)) continue;
      const deps = children.get(waiter) ?? [];
      deps.push(goal.id);
      children.set(waiter, deps);
      hasParent.add(goal.id);
    }
  }

  const order = (ids: string[]): string[] =>
    ids.sort((left, right) => {
      const a = goalById.get(left);
      const b = goalById.get(right);
      if (a === undefined || b === undefined) return left.localeCompare(right);
      return STATUS_ORDER[a.status] - STATUS_ORDER[b.status] || left.localeCompare(right);
    });

  const childrenById = new Map<string, readonly string[]>();
  for (const [parent, deps] of children) childrenById.set(parent, order(deps));

  // Requested roots first (in the coordinator's order), then any orphan goal
  // nothing waits on, so the forest always covers every listed goal.
  const roots = coordinator.roots.filter((root) => goalById.has(root));
  const inRoots = new Set(roots);
  const orphans = order(
    [...goalById.keys()].filter((id) => !hasParent.has(id) && !inRoots.has(id))
  );
  return { goalById, childrenById, roots: [...roots, ...orphans], counts };
}

/// Pre-order walk of the rows the forest currently shows. `isCollapsed`
/// answers per goal id (a collapsed goal keeps its own row but hides its
/// descendants); a predicate rather than a set because the caller keys its
/// collapse state per coordinator. A goal already expanded earlier in the walk
/// is not re-entered (diamond/cycle guard), matching the patched nix's own
/// human forest rendering.
export function flattenGoalForest(
  forest: GoalForest,
  isCollapsed: (id: string) => boolean
): readonly GoalRow[] {
  const rows: GoalRow[] = [];
  const expanded = new Set<string>();
  const walk = (id: string, depth: number): void => {
    const goal = forest.goalById.get(id);
    if (goal === undefined) return;
    const childIds = forest.childrenById.get(id) ?? [];
    const repeat = expanded.has(id);
    rows.push({ goal, depth, hasChildren: childIds.length > 0, repeat });
    if (repeat || isCollapsed(id)) return;
    expanded.add(id);
    for (const child of childIds) walk(child, depth + 1);
  };
  for (const root of forest.roots) walk(root, 0);
  return rows;
}
