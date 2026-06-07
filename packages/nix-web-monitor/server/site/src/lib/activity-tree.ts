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
///
/// Sibling rows that share the same label are then folded into one row carrying
/// a `×N` count. Nix copies a single flake input to the store once per consumer
/// and emits an identical activity each time, so an ungrouped view is a wall of
/// dozens of identical "copying …/source.tar.gz to the store" lines that buries
/// the builds and downloads an operator actually cares about. The fold keeps one
/// row per distinct label per parent and pushes the real work back into view.

import {
  activityKind,
  progressUnit,
  splitDerivation,
  type DerivationParts,
  type ProgressUnit
} from '$lib/format';
import type { ActivityNode, ActivityStatus, BuildNode, BuildStatus } from '$lib/types';

/// Byte/item progress for a copy or download row, summed across a folded group.
/// `unit` says whether `done`/`expected` are bytes (a store copy or substituter
/// download) or item counts. Only present when the activity reports measurable
/// progress; a build or a copy before its first progress event has none.
export type RowProgress = Readonly<{
  done: number;
  expected: number;
  unit: ProgressUnit;
}>;

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
  /// Number of identical sibling activities folded into this row. `1` means a
  /// lone row; greater means the label repeated and the row shows `×N`.
  count: number;
  /// Earliest start across the folded group; the lone start when `count` is 1.
  startedAtMs: number;
  /// Latest stop across the folded group, or `null` while any member still runs.
  stoppedAtMs: number | null;
  /// Copy/download progress, summed across the folded group, or `null` when the
  /// row reports nothing measurable. Lets the row show how much data is moving.
  progress: RowProgress | null;
  /// Total bytes being copied to the store, summed across the folded group, or
  /// `null` when nothing in the group was measured. Set for Nix's local source
  /// copy, which it reports without `progress`, so the row can still show a size.
  sizeBytes: number | null;
}>;

export type ActivityTree = Readonly<{
  rowMeta: ReadonlyMap<number, ActivityRowMeta>;
  childrenById: ReadonlyMap<number, readonly number[]>;
  roots: readonly number[];
  shown: number;
  hidden: number;
}>;

/// Stable per-row descriptor, computed once and reused for both the grouping
/// key and the rendered `rowMeta`. The key is what makes two sibling rows fold:
/// same kind, same identifying payload (derivation name for builds, display text
/// otherwise).
type RowDescriptor = Readonly<{
  kind: string;
  derivation: DerivationParts | null;
  text: string;
  key: string;
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

  const rawChildren = new Map<number, number[]>();
  const rawRoots: number[] = [];
  for (const activity of visible) {
    const parent = displayParent(activity);
    if (parent === null) {
      rawRoots.push(activity.id);
    } else {
      const siblings = rawChildren.get(parent) ?? [];
      siblings.push(activity.id);
      rawChildren.set(parent, siblings);
    }
  }

  const byStartTick = (left: number, right: number): number => {
    const a = byId.get(left);
    const b = byId.get(right);
    return (a?.startedTick ?? 0) - (b?.startedTick ?? 0);
  };

  const descriptor = new Map<number, RowDescriptor>(
    visible.map((activity) => {
      const isBuild = buildIds.has(activity.id);
      const derivation = isBuild && activity.build !== null ? splitDerivation(activity.build) : null;
      const kind = activityKind(activity.activityType.name, activity.text);
      const text = activity.phase ?? activity.text;
      return [activity.id, { kind, derivation, text, key: `${kind} ${derivation?.name ?? text}` }];
    })
  );

  /// Members folded into each kept representative, in start order. Built top
  /// down so a representative's children are the merged children of every member
  /// it absorbed, themselves folded recursively.
  const members = new Map<number, number[]>();
  const childrenById = new Map<number, number[]>();

  /// Fold one ordered sibling list: the first activity for a given key is kept,
  /// later identical ones are absorbed into it. Returns the kept order.
  function fold(ids: number[]): number[] {
    const repByKey = new Map<string, number>();
    const kept: number[] = [];
    for (const id of ids) {
      const key = descriptor.get(id)?.key ?? String(id);
      const rep = repByKey.get(key);
      if (rep === undefined) {
        repByKey.set(key, id);
        kept.push(id);
        members.set(id, [id]);
      } else {
        members.get(rep)?.push(id);
      }
    }
    return kept;
  }

  /// Fold a level, then recurse into each kept row over the merged children of
  /// all its members. A representative inherits every absorbed member's subtree.
  function descend(ids: number[]): number[] {
    const kept = fold(ids.toSorted(byStartTick));
    for (const rep of kept) {
      const kids = (members.get(rep) ?? [])
        .flatMap((member) => rawChildren.get(member) ?? [])
        .toSorted(byStartTick);
      const keptKids = descend(kids);
      if (keptKids.length > 0) childrenById.set(rep, keptKids);
    }
    return kept;
  }

  const roots = descend(rawRoots);

  /// Sum the `done`/`expected` counters across a folded group, classifying the
  /// unit from the activity type (bytes for copies and downloads, items
  /// otherwise). `null` when nothing in the group has measurable expected work,
  /// so a build or a copy before its first progress event shows no bar.
  function groupProgress(group: number[], typeName: string): RowProgress | null {
    let done = 0;
    let expected = 0;
    for (const member of group) {
      const progress = byId.get(member)?.progress;
      if (progress === null || progress === undefined) continue;
      done += progress.done;
      expected += progress.expected;
    }
    return expected > 0 ? { done, expected, unit: progressUnit(typeName) } : null;
  }

  /// Sum the measured copy size across a folded group. `null` when no member was
  /// measured, so a row that is not a "copying … to the store" activity (or one
  /// whose measurement has not landed yet) shows no size badge.
  function groupSize(group: number[]): number | null {
    let total = 0;
    let measured = false;
    for (const member of group) {
      const size = byId.get(member)?.sizeBytes;
      if (size === null || size === undefined) continue;
      total += size;
      measured = true;
    }
    return measured ? total : null;
  }

  const rowMeta = new Map<number, ActivityRowMeta>();
  for (const [rep, group] of members) {
    const desc = descriptor.get(rep);
    const activity = byId.get(rep);
    if (desc === undefined || activity === undefined) continue;

    if (group.length === 1) {
      rowMeta.set(rep, {
        id: rep,
        kind: desc.kind,
        state: buildStatusByActivity.get(rep) ?? activity.status,
        derivation: desc.derivation,
        text: desc.text,
        count: 1,
        startedAtMs: activity.startedAtMs,
        stoppedAtMs: activity.stoppedAtMs,
        progress: groupProgress(group, activity.activityType.name),
        sizeBytes: groupSize(group)
      });
      continue;
    }

    // A folded group only ever holds non-build activities (builds have unique
    // derivation keys), so its state is the simple running/stopped union and
    // its span runs from the earliest start to the latest stop.
    const running = group.some((member) => byId.get(member)?.status === 'running');
    const startedAtMs = Math.min(
      ...group.map((member) => byId.get(member)?.startedAtMs ?? activity.startedAtMs)
    );
    const stoppedAtMs = running
      ? null
      : Math.max(...group.map((member) => byId.get(member)?.stoppedAtMs ?? activity.startedAtMs));
    rowMeta.set(rep, {
      id: rep,
      kind: desc.kind,
      state: running ? 'running' : 'stopped',
      derivation: desc.derivation,
      text: desc.text,
      count: group.length,
      startedAtMs,
      stoppedAtMs,
      progress: groupProgress(group, activity.activityType.name),
      sizeBytes: groupSize(group)
    });
  }

  return {
    rowMeta,
    childrenById,
    roots,
    shown: rowMeta.size,
    hidden: activities.length - rowMeta.size
  };
}
