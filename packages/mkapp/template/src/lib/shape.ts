// Reading a stale storage mirror back without blanking the page.
//
// THE BUG THIS EXISTS FOR (index ENG-11106). The store used to rehydrate with
// `JSON.parse(raw) as AppState`. That cast is a lie the moment the store's shape
// changes: the mirror in an already-open page was written by the PREVIOUS shape,
// components index into fields that are now undefined, and the page goes blank.
// Every layer reports success -- the gate is green, the promote succeeded, the
// dev server is healthy -- because the failure is at runtime in someone's
// browser. A green check and an empty DOM.
//
// WHY NOT JUST VERSION THE STORAGE KEY. Bumping `mkapp:state` to `:v2` on every
// shape change does stop the crash, but only by throwing the reader's page away
// and reseeding. That is the same blank page with a different cause, and it
// happens once per shape change forever.
//
// WHAT THIS DOES INSTEAD. Reconcile field by field against the current shape:
// keep what is valid, fill what is missing from the fallback, drop what is not
// recognised, and validate array elements one at a time so one bad section does
// not cost the other twenty. The property that matters is that an ADDITIVE shape
// change is absorbed rather than fatal -- a reader who has had the page open all
// afternoon keeps their content and gains the new field's default in the same
// promote. Only a root that is not an object at all falls back to the seed.
//
// Pure, and covered by `shape.test.ts` under the gate, because a bug in here is
// exactly the class that ships green.

// Relative imports carry the `.ts` extension throughout lib/: `node --test`
// strips types without rewriting specifiers, so an extensionless import breaks
// the moment a module is pulled into a test. One convention, no special cases.
import type { Entry, HistoryState } from './history.ts';
import type { AppState, Section } from './plan.ts';

/** A plain object, the only thing any of these readers descend into. */
function record(value: unknown): Record<string, unknown> | null {
  return typeof value === 'object' && value !== null && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : null;
}

function str(value: unknown, fallback: string): string {
  return typeof value === 'string' ? value : fallback;
}

function bool(value: unknown, fallback: boolean): boolean {
  return typeof value === 'boolean' ? value : fallback;
}

function num(value: unknown, fallback: number): number {
  return typeof value === 'number' && Number.isFinite(value) ? value : fallback;
}

/** Read each element with `item`, dropping the ones it rejects. */
function list<T>(value: unknown, item: (raw: unknown) => T | null): T[] | null {
  if (!Array.isArray(value)) return null;
  const out: T[] = [];
  for (const raw of value) {
    const parsed = item(raw);
    if (parsed !== null) out.push(parsed);
  }
  return out;
}

/**
 * One section. `id` is the only field with no sensible default -- a section
 * without one cannot be addressed by a mutator or keyed by the `{#each}`, so it
 * is dropped rather than invented.
 */
function section(raw: unknown): Section | null {
  const source = record(raw);
  if (!source) return null;
  const id = str(source.id, '');
  if (!id) return null;
  return {
    id,
    title: str(source.title, id),
    loading: bool(source.loading, false),
    body: str(source.body, ''),
  };
}

/**
 * The app state from a storage mirror, reconciled against `fallback`.
 *
 * `fallback` is the seed, so every field it does not recover comes back as the
 * value a fresh page would have shown.
 */
export function reconcileState(raw: unknown, fallback: AppState): AppState {
  const source = record(raw);
  if (!source) return fallback;
  return {
    status: str(source.status, fallback.status),
    done: bool(source.done, fallback.done),
    sections: list(source.sections, section) ?? fallback.sections,
  };
}

/** The known mutator names. An entry naming anything else is from a newer build. */
const KINDS = new Set(['seed', 'set', 'add', 'remove', 'say', 'narrate', 'reset', 'external']);

/**
 * One history entry.
 *
 * `changes` is passed through unvalidated ON PURPOSE, beyond being an array: it
 * is opaque JSON that only `applyChange` interprets, and `applyChange` already
 * skips anything whose path does not resolve. Type-checking it here would mean
 * a second copy of the `Change` union that has to be kept in step with the
 * first, to reject values the consumer already tolerates.
 */
function entry(raw: unknown): Entry | null {
  const source = record(raw);
  if (!source) return null;
  if (typeof source.seq !== 'number' || !Array.isArray(source.changes)) return null;
  const kind = str(source.kind, '');
  if (!KINDS.has(kind)) return null;
  const actor = record(source.actor);
  const actorKind = str(actor?.kind, 'agent');
  return {
    seq: source.seq,
    ts: num(source.ts, 0),
    actor: {
      kind: actorKind === 'human' ? 'human' : 'agent',
      label: str(actor?.label, 'unknown'),
    },
    kind: kind as Entry['kind'],
    target: str(source.target, ''),
    label: str(source.label, ''),
    changes: source.changes as Entry['changes'],
  };
}

/**
 * The history log from its own storage mirror.
 *
 * An absent or unreadable log is an EMPTY history, never a fallback to the
 * content's seed: losing the log must cost the log and nothing else. That is
 * also why it lives under its own key.
 */
export function reconcileHistory(raw: unknown): HistoryState {
  const source = record(raw);
  if (!source) return { entries: [], head: '', open: false };
  return {
    entries: list(source.entries, entry) ?? [],
    head: str(source.head, ''),
    open: bool(source.open, false),
  };
}

/** `JSON.parse` that answers `undefined` instead of throwing. */
export function parseJson(raw: string | null): unknown {
  if (raw === null) return undefined;
  try {
    return JSON.parse(raw) as unknown;
  } catch {
    return undefined;
  }
}
