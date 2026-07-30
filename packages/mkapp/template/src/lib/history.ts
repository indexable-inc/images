// The version history: an append-only log of every change to the page.
//
// This module is deliberately pure. It knows nothing about Svelte, runes, the
// DOM or sessionStorage: it maps plain JSON and a list of entries to another
// plain JSON. That is what makes it unit-testable without a browser, and it is
// what `store.svelte.ts` records into.
//
// WHY A HAND-WRITTEN LOG AND NOT A CRDT. A CRDT's oplog would give timestamps
// and checkout for free, but its ops sit BELOW the level a reader cares about:
// "insert key `status` into container cid:..." rather than "the CVE-scan
// finding went from open to running". Recovering the second from the first is
// guesswork against information the op layer already threw away. mkapp's
// mutations are named and targeted at the call site, so the sentence is already
// in hand -- there is nothing to recover. And attribution is not a CRDT feature
// at all: a peer id is random per document, so "who" is application data either
// way. What a CRDT would actually buy here is merge, and there is one writer.

/** Plain JSON. The log only ever holds values that survive a storage round-trip. */
export type Json = string | number | boolean | null | Json[] | { [key: string]: Json };

/** A location in the state: object keys and array indices, outermost first. */
export type Path = (string | number)[];

/**
 * One reversible edit to the state.
 *
 * Three ops rather than one, because a `set` cannot express an array insert
 * without carrying the whole array as its before-image. `add` on a page with
 * thirty sections would then cost thirty sections of log per entry.
 */
export type Change =
  /**
   * Replace the value at `path`. `before`/`after` absent means the key was, or
   * becomes, absent -- which `JSON.stringify` preserves by dropping the key, so
   * the distinction survives storage. An empty `path` replaces the whole root.
   */
  | { op: 'set'; path: Path; before?: Json; after?: Json }
  /** Insert `value` at `index` of the array at `path`. */
  | { op: 'insert'; path: Path; index: number; value: Json }
  /** Remove the element at `index` of the array at `path`. `value` is kept so this inverts. */
  | { op: 'delete'; path: Path; index: number; value: Json };

/** Software or a person: the first thing a reader of a change asks. */
export type ActorKind = 'agent' | 'human';

export interface Actor {
  kind: ActorKind;
  /** What to call it: a subagent's name, a person's handle. */
  label: string;
}

/**
 * Which mutation produced an entry.
 *
 * `seed` is the page as first loaded, so the history has a floor. `external` is
 * a change that reached the state without going through a mutator -- see
 * `store.svelte.ts`. Everything else is the name of the function that ran.
 */
export type EntryKind = 'seed' | 'set' | 'add' | 'remove' | 'say' | 'narrate' | 'reset' | 'external';

/** One row of the history: who changed what, when, and how to undo it. */
export interface Entry {
  /** Monotonic within a session; the stable handle a row is addressed by. */
  seq: number;
  /** Wall clock in milliseconds. */
  ts: number;
  actor: Actor;
  kind: EntryKind;
  /**
   * The id of the section this touched, for the UI to scroll to. Empty for a
   * page-level change. Deliberately separate from `changes`, which addresses by
   * array INDEX: an index is only valid at this exact point in the walk, so it
   * is right for reconstruction and wrong for a link.
   */
  target: string;
  /** The one-line "what changed". */
  label: string;
  /** What it did, in order. Empty is not recorded -- see `record` in the store. */
  changes: Change[];
}

/** The log, plus the content it accounts for. */
export interface HistoryState {
  entries: Entry[];
  /**
   * Whether the panel is showing.
   *
   * Durable rather than component-local, because a promote remounts the
   * component: a reader who opened the history to watch an agent work would
   * otherwise lose it every seven seconds, during exactly the activity it
   * exists to show.
   */
  open: boolean;
  /**
   * `JSON.stringify` of the state as of the last recorded entry.
   *
   * This is what makes an out-of-band write detectable: anything that changes
   * the state without recording an entry leaves `head` stale, and the mismatch
   * is both the alarm and the before-image of the change that was missed. It is
   * persisted rather than held in a module variable so the check still works
   * across a full reload, where the two storage keys can disagree.
   */
  head: string;
}

/** A deep copy, and the only cloning this module does. */
export function clone<T extends Json>(value: T): T {
  return JSON.parse(JSON.stringify(value)) as T;
}

/** The change that exactly undoes `change`. */
export function invert(change: Change): Change {
  switch (change.op) {
    case 'set':
      return { op: 'set', path: change.path, before: change.after, after: change.before };
    case 'insert':
      return { op: 'delete', path: change.path, index: change.index, value: change.value };
    case 'delete':
      return { op: 'insert', path: change.path, index: change.index, value: change.value };
  }
}

/**
 * A copy of `value` that shares no structure with it.
 *
 * Load-bearing, and the source of a real bug caught by `history.test.ts`. A
 * change's `value`/`after` belongs to the LOG, which is immutable by contract.
 * Writing it into the state by reference aliases the two, so the next mutation
 * of that section reaches through and rewrites the recorded before-image --
 * history silently changing to agree with the present, which is the one failure
 * a version history must not have. Copying at the boundary makes the aliasing
 * impossible rather than merely unlikely.
 *
 * Primitives are returned as they are: they cannot be aliased, and a JSON
 * round-trip on every string set would be pure waste.
 */
function detach(value: Json): Json {
  return value === null || typeof value !== 'object' ? value : clone(value);
}

/** The container at `path`, or undefined if the path does not resolve. */
function walk(root: Json, path: Path): Json | undefined {
  let node: Json | undefined = root;
  for (const step of path) {
    if (node === null || typeof node !== 'object') return undefined;
    node = (node as Record<string | number, Json>)[step];
  }
  return node;
}

/**
 * Apply one change, mutating `root` and returning the (possibly new) root.
 *
 * A change whose path no longer resolves is SKIPPED rather than thrown. That
 * only happens if the log and the state have already diverged, and in that case
 * a history panel that still renders is worth more than one that throws.
 */
export function applyChange(root: Json, change: Change): Json {
  if (change.op === 'set') {
    if (change.path.length === 0) return change.after === undefined ? null : detach(change.after);
    const parent = walk(root, change.path.slice(0, -1));
    if (parent === null || typeof parent !== 'object') return root;
    const key = change.path[change.path.length - 1];
    const slot = parent as Record<string | number, Json>;
    if (change.after === undefined) delete slot[key];
    else slot[key] = detach(change.after);
    return root;
  }
  const target = walk(root, change.path);
  if (!Array.isArray(target)) return root;
  if (change.op === 'insert') target.splice(change.index, 0, detach(change.value));
  else target.splice(change.index, 1);
  return root;
}

/**
 * The state as it stood just after `entries[upto]`.
 *
 * Walks INVERSES BACKWARDS from the current state rather than replaying forwards
 * from a seed, and that direction is the load-bearing choice. Replaying forwards
 * would need the seed to be stable, but the seed is a source file an agent
 * edits, so an old log replayed against a new seed reconstructs a page that
 * never existed. Walking backwards is anchored on the state actually in front of
 * the reader, so it is exact no matter what the seed has since become.
 *
 * It is also why trimming the log needs no baseline snapshot: the current state
 * IS the anchor, so the oldest retained entry is always reachable, and the only
 * thing trimming costs is the states older than it -- exactly what it decided to
 * discard.
 *
 * Index-addressed paths are valid here for the same reason: each entry's
 * indices were computed against the state immediately before it, which is
 * precisely the state this loop is holding when it applies that entry's inverse.
 */
export function stateAt<T extends Json>(current: T, entries: Entry[], upto: number): T {
  let root: Json = clone(current);
  for (let i = entries.length - 1; i > upto; i--) {
    const changes = entries[i].changes;
    // Within an entry too: the last change is the first to be undone.
    for (let j = changes.length - 1; j >= 0; j--) {
      root = applyChange(root, invert(changes[j]));
    }
  }
  return root as T;
}

/**
 * The changes needed to bring `current` in line with `patch`, and nothing more.
 *
 * THE LOAD-BEARING FUNCTION for the log's central invariant: the history records
 * state TRANSITIONS, not calls. mkapp's update file is a normal module, so it
 * re-runs top to bottom on every promote, and its statements are deliberately
 * idempotent so a caller can leave them in place. A log that appended per call
 * would therefore grow by the whole file every seven seconds and read as
 * hundreds of rows saying nothing happened. Returning an EMPTY array for a patch
 * that changes nothing is what makes a re-run cost exactly zero entries, and it
 * is pure so that property is testable without a browser.
 *
 * A key whose value is `undefined` in the patch is "not mentioned", not "set to
 * undefined": that is what makes `Partial<T>` mean what a caller expects.
 */
export function diffFields(
  current: Record<string, Json>,
  patch: Record<string, Json | undefined>,
  path: Path,
  skip: readonly string[] = [],
): Change[] {
  const changes: Change[] = [];
  for (const key of Object.keys(patch)) {
    if (skip.includes(key)) continue;
    const after = patch[key];
    if (after === undefined) continue;
    const before = current[key];
    if (before === after) continue;
    changes.push({ op: 'set', path: [...path, key], before, after });
  }
  return changes;
}

/** A short, safe rendering of a value for a history row. */
export function formatValue(value: Json | undefined): string {
  if (value === undefined) return '∅';
  if (value === null) return 'null';
  if (typeof value === 'string') return value.length > 24 ? `${value.slice(0, 23)}…` : value;
  if (typeof value === 'number' || typeof value === 'boolean') return String(value);
  return Array.isArray(value) ? `[${value.length}]` : '{…}';
}

/** A bare relative age: `now`, `4m`, `2h`. */
export function shortAge(ts: number, now: number): string {
  const seconds = Math.max(0, Math.round((now - ts) / 1000));
  if (seconds < 5) return 'now';
  if (seconds < 60) return `${seconds}s`;
  const minutes = Math.round(seconds / 60);
  if (minutes < 60) return `${minutes}m`;
  const hours = Math.round(minutes / 60);
  return hours < 24 ? `${hours}h` : `${Math.round(hours / 24)}d`;
}
