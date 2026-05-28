/// Builds a renderable hierarchy from the flat activity list.
///
/// Nix's internal-json stream gives each activity a `parent` pointer, which
/// forms the *activity* tree (a `builds` coordinator parents the individual
/// `build` activities it spawns, a `realise` parents its substitutions, and so
/// on). That tree is the only dependency structure the protocol exposes: there
/// is no edge that says "derivation A needs derivation B", so this is a faithful
/// view of what Nix reports, not a reconstructed derivation DAG.
///
/// Uninteresting intermediate activities (no text, no phase, not a build) are
/// dropped, and their visible descendants are re-parented onto the nearest
/// interesting ancestor. That keeps the rendered tree connected without empty
/// filler rows.

import { activityKind, splitDerivation, type DerivationParts } from '$lib/format';
import type { ActivityNode, ActivityStatus, BuildNode, BuildStatus } from '$lib/types';

export type ActivityRowMeta = Readonly<{
  id: number;
  kind: string;
  /// Build status when this activity is a derivation build, otherwise the
  /// raw activity status. Drives the status dot colour.
  state: BuildStatus | ActivityStatus;
  /// Present only for build activities; lets the row dim the store-path hash.
  derivation: DerivationParts | null;
  /// Display text for non-build rows (phase preferred over raw text).
  text: string;
  startedAtMs: number;
  stoppedAtMs: number | null;
}>;

export type ActivityTree = Readonly<{
  rowMeta: ReadonlyMap<number, ActivityRowMeta>;
  childrenById: ReadonlyMap<number, readonly number[]>;
  roots: readonly number[];
  shown: number;
  hidden: number;
}>;

function isInteresting(activity: ActivityNode, buildIds: ReadonlySet<number>): boolean {
  // Build activities are always worth a row, even when Nix gives them empty
  // text. Everything else needs a phase or text to earn one.
  return buildIds.has(activity.id) || activity.text.length > 0 || activity.phase !== null;
}

export function buildActivityTree(activities: ActivityNode[], builds: BuildNode[]): ActivityTree {
  const byId = new Map(activities.map((activity) => [activity.id, activity]));
  const buildIds = new Set(
    builds.flatMap((build) => (build.activityId === null ? [] : [build.activityId]))
  );
  const buildStatusByActivity = new Map(
    builds.flatMap((build) => (build.activityId === null ? [] : [[build.activityId, build.status]]))
  );

  const visible = activities.filter((activity) => isInteresting(activity, buildIds));
  const visibleIds = new Set(visible.map((activity) => activity.id));

  /// Nearest ancestor that is itself visible, or null when this row is a root.
  function displayParent(activity: ActivityNode): number | null {
    let parent = activity.parent;
    while (parent !== null) {
      if (visibleIds.has(parent)) return parent;
      const next = byId.get(parent);
      if (next === undefined) break;
      parent = next.parent;
    }
    return null;
  }

  const childrenById = new Map<number, number[]>();
  const roots: number[] = [];
  for (const activity of visible) {
    const parent = displayParent(activity);
    if (parent === null) {
      roots.push(activity.id);
    } else {
      const siblings = childrenById.get(parent) ?? [];
      siblings.push(activity.id);
      childrenById.set(parent, siblings);
    }
  }

  const byStartTick = (left: number, right: number): number => {
    const a = byId.get(left);
    const b = byId.get(right);
    return (a?.startedTick ?? 0) - (b?.startedTick ?? 0);
  };
  roots.sort(byStartTick);
  for (const siblings of childrenById.values()) siblings.sort(byStartTick);

  const rowMeta = new Map<number, ActivityRowMeta>(
    visible.map((activity) => {
      const isBuild = buildIds.has(activity.id);
      return [
        activity.id,
        {
          id: activity.id,
          kind: activityKind(activity.activityType.name, activity.text),
          state: buildStatusByActivity.get(activity.id) ?? activity.status,
          derivation: isBuild && activity.build !== null ? splitDerivation(activity.build) : null,
          text: activity.phase ?? activity.text,
          startedAtMs: activity.startedAtMs,
          stoppedAtMs: activity.stoppedAtMs
        }
      ];
    })
  );

  return {
    rowMeta,
    childrenById,
    roots,
    shown: visible.length,
    hidden: activities.length - visible.length
  };
}
