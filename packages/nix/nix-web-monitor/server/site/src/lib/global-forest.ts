/// Reconstructs the machine-wide derivation forest from the flat active-goal
/// list. Every active goal carries `why.chain` (root-first store paths ending
/// at the goal itself), so the forest is rebuildable client-side with no
/// protocol change: chains merge wherever they share a node, keyed by store
/// path. Chain hops with no active goal of their own become skeleton nodes.
///
/// Known limitation: the status dir records only *active* goals plus their
/// ancestor chains, so waiting/queued siblings are invisible until it records
/// the full goal graph (sibling sub-issue of #2484).

import { splitDerivation } from '$lib/format';
import type { GlobalBuild } from '$lib/types';

/// The store path a goal identifies: the drv for a build, the store path for a
/// substitution. Either can be null on a drifted entry, so fall back to the
/// other and finally to a placeholder rather than keying on `null`.
export function goalPath(build: GlobalBuild): string {
  return build.drvPath ?? build.storePath ?? '(unknown)';
}

export type GlobalForest = Readonly<{
  /// Active goals per node, oldest first. A path with no entry here is a
  /// skeleton ancestor: a chain hop nothing is actively building.
  goalsByPath: ReadonlyMap<string, readonly GlobalBuild[]>;
  /// Children per node. Name-ordered (not start-ordered) so rows keep a stable
  /// order across the two-second re-polls as goals come and go.
  childrenByPath: ReadonlyMap<string, readonly string[]>;
  /// The requested derivations (chain heads nothing else wants), name-ordered.
  roots: readonly string[];
}>;

/// Root-first chain from the requested derivation to this goal, normalized to
/// end at the goal itself (the wire chain usually includes it, but a sparse
/// entry may carry only `rootDrvPath`, or nothing).
function chainTo(build: GlobalBuild): readonly string[] {
  const path = goalPath(build);
  const chain = build.why.chain;
  if (chain.length > 0) {
    return chain[chain.length - 1] === path ? chain : [...chain, path];
  }
  const root = build.why.rootDrvPath;
  return root === null || root === path ? [path] : [root, path];
}

/// Display order: package name first, full path as the tiebreak, so identical
/// names (different hashes) stay stably ordered.
function byName(left: string, right: string): number {
  return (
    splitDerivation(left).name.localeCompare(splitDerivation(right).name) ||
    left.localeCompare(right)
  );
}

export function buildGlobalForest(builds: readonly GlobalBuild[]): GlobalForest {
  const goalsByPath = new Map<string, GlobalBuild[]>();
  const childSets = new Map<string, Set<string>>();
  const isChild = new Set<string>();
  const paths = new Set<string>();

  for (const build of builds) {
    const path = goalPath(build);
    const goals = goalsByPath.get(path) ?? [];
    goals.push(build);
    goalsByPath.set(path, goals);

    const chain = chainTo(build);
    for (const hop of chain) paths.add(hop);
    for (let i = 1; i < chain.length; i += 1) {
      const parent = chain[i - 1];
      const child = chain[i];
      // A drifted self-hop would nest a node under itself forever.
      if (parent === child) continue;
      const children = childSets.get(parent) ?? new Set<string>();
      children.add(child);
      childSets.set(parent, children);
      isChild.add(child);
    }
  }

  // The status dir keys entries by `<path>-<pid>`, so one path can carry a
  // goal per daemon worker; oldest first so the row leads with the
  // longest-running one.
  for (const goals of goalsByPath.values()) {
    goals.sort((a, b) => (a.startTime ?? 0) - (b.startTime ?? 0));
  }

  const childrenByPath = new Map<string, readonly string[]>();
  for (const [parent, children] of childSets) {
    childrenByPath.set(parent, [...children].sort(byName));
  }

  const roots = [...paths].filter((path) => !isChild.has(path)).sort(byName);

  // Contradictory chains (A above B in one goal, B above A in another) mark
  // both as children and orphan the whole component. Promote any active goal
  // the roots cannot reach so a drifted entry stays visible; the renderer's
  // ancestor guard breaks the cycle below it.
  const reachable = new Set<string>();
  const visit = (path: string): void => {
    if (reachable.has(path)) return;
    reachable.add(path);
    for (const child of childrenByPath.get(path) ?? []) visit(child);
  };
  for (const root of roots) visit(root);
  // Promote incrementally: the first orphan's subtree covers its cycle mates,
  // so they are not promoted twice.
  const orphans: string[] = [];
  for (const path of [...goalsByPath.keys()].sort(byName)) {
    if (reachable.has(path)) continue;
    orphans.push(path);
    visit(path);
  }

  return { goalsByPath, childrenByPath, roots: [...roots, ...orphans] };
}
