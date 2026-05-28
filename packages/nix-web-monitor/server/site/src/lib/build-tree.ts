/// Turns the flat build list plus the dependency edge set into a renderable
/// forest. Each edge `from -> to` means `from` directly requires `to`, so a
/// node's children are its dependencies and the roots are the top-level
/// targets (built derivations nothing else depends on).
///
/// The edge set is already transitively reduced server-side, so the tree is
/// minimal. A diamond still unfolds its shared dependency under each dependent;
/// that mirrors how `nix-output-monitor` prints and is fine for the small
/// graphs a single build produces.

import type { BuildNode, DerivationEdge } from '$lib/types';

export type BuildTree = Readonly<{
  nodeByDrv: ReadonlyMap<string, BuildNode>;
  /// Direct dependencies per derivation, in display order.
  childrenByDrv: ReadonlyMap<string, readonly string[]>;
  /// Top-level derivations nothing else depends on.
  roots: readonly string[];
  /// Whether any dependency edge survived. Distinguishes "no edges resolved
  /// yet" from a genuinely independent set of builds in the empty state.
  hasEdges: boolean;
}>;

export function buildDependencyTree(
  builds: readonly BuildNode[],
  dependencies: readonly DerivationEdge[],
  compare: (left: BuildNode, right: BuildNode) => number
): BuildTree {
  const nodeByDrv = new Map(builds.map((build) => [build.derivation, build]));
  const children = new Map<string, string[]>();
  const hasParent = new Set<string>();

  for (const { from, to } of dependencies) {
    // Ignore edges whose endpoints have no build row; the server only emits
    // edges between built derivations, but this keeps the view honest if that
    // ever drifts.
    if (!nodeByDrv.has(from) || !nodeByDrv.has(to)) continue;
    const deps = children.get(from) ?? [];
    deps.push(to);
    children.set(from, deps);
    hasParent.add(to);
  }

  const order = (drvs: readonly string[]): string[] =>
    drvs.toSorted((left, right) => {
      const a = nodeByDrv.get(left);
      const b = nodeByDrv.get(right);
      return a === undefined || b === undefined ? left.localeCompare(right) : compare(a, b);
    });

  const childrenByDrv = new Map<string, readonly string[]>();
  for (const [from, deps] of children) childrenByDrv.set(from, order(deps));

  const roots = order(
    builds.map((build) => build.derivation).filter((drv) => !hasParent.has(drv))
  );

  return { nodeByDrv, childrenByDrv, roots, hasEdges: dependencies.length > 0 };
}
