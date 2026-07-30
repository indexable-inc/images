// The document model, and what each mutation actually changes.
//
// Pure by design: every planner takes the CURRENT state as plain JSON and
// returns the `Change[]` that mutation would make, touching nothing. The
// reactive store is then a thin shell that applies them and records them.
//
// The split exists so the update surface's central promise is testable without a
// browser. That promise is COMPOSITE IDEMPOTENCE: the update file re-runs top to
// bottom on every promote, so running it twice must leave the same state AND the
// same history as running it once -- not just per statement, but for the
// sequences callers actually write. `add` followed by `say` on one section is
// the common shape, and getting it wrong is invisible in a unit test of either
// one alone. It cost two spurious entries per promote before `plan.test.ts`
// pinned it.

import { type Change, type Json, clone, diffFields } from './history.ts';

export type Section = {
  id: string;
  title: string;
  /** True while the agent is still generating this section: skeletons show. */
  loading: boolean;
  body: string;
};

export type AppState = {
  /** One line: what the agent is doing right now. */
  status: string;
  /** Set once the whole task is finished. */
  done: boolean;
  sections: Section[];
};

/** The page as a reader first receives it. */
export function initialState(): AppState {
  return {
    status: 'waiting for the agent',
    done: false,
    sections: [
      {
        id: 'welcome',
        title: 'Welcome',
        loading: false,
        body:
          'Scaffolded by mkapp. The agent edits staging/ and this page hot ' +
          'reloads each promoted change without losing this store. Press H ' +
          'for the history of every change to this page.',
      },
    ],
  };
}

/** `id` is how a section is addressed, so it is never patchable. */
const IMMUTABLE = ['id'] as const;

function indexOf(state: AppState, id: string): number {
  return state.sections.findIndex((section) => section.id === id);
}

/**
 * Change part of one section.
 *
 * An unknown id plans nothing rather than throwing: a statement naming a section
 * that has since been removed should not blank the page for a reader.
 */
export function planSet(state: AppState, id: string, patch: Partial<Section>): Change[] {
  const index = indexOf(state, id);
  if (index < 0) return [];
  // One contained cast, at the boundary between the typed surface and the
  // untyped log. TypeScript cannot relate `patch[key]` to `Section[key]` when
  // `key` ranges over a union, and the alternative is a per-field branch
  // repeating the same lines for every field of every future section type.
  return diffFields(
    state.sections[index] as unknown as Record<string, Json>,
    patch as unknown as Record<string, Json | undefined>,
    ['sections', index],
    IMMUTABLE,
  );
}

/**
 * File a section, if it is not already there.
 *
 * INSERT-IF-ABSENT, NOT UPSERT, and the distinction is the whole reason this
 * function is safe to leave in an update file. An upsert re-applies the literal
 * in the caller's source on every promote, which silently REVERTS anything that
 * has happened to that section since -- most obviously a `say` that appended to
 * its body, which then re-appends, so each promote costs two entries and the
 * page flickers. Insert-if-absent composes with every other mutator instead.
 *
 * To change a section that already exists, use `planSet`.
 */
export function planAdd(
  state: AppState,
  section: Section,
  where: 'top' | 'bottom' = 'bottom',
): Change[] {
  if (indexOf(state, section.id) >= 0) return [];
  return [
    {
      op: 'insert',
      path: ['sections'],
      index: where === 'top' ? 0 : state.sections.length,
      // Cloned so a caller that keeps and later mutates its literal cannot reach
      // into the log. `applyChange` clones again on the way out, guarding the
      // other direction.
      value: clone(section as unknown as Json),
    },
  ];
}

/** Drop a section. An id that is not there is not an error. */
export function planRemove(state: AppState, id: string): Change[] {
  const index = indexOf(state, id);
  if (index < 0) return [];
  return [
    {
      op: 'delete',
      path: ['sections'],
      index,
      // The removed value rides along so the change inverts: this is what lets a
      // reader scrub back to before a deletion and see the section again.
      value: clone(state.sections[index] as unknown as Json),
    },
  ];
}

/**
 * Append a paragraph to a section's body, once.
 *
 * Re-running adds nothing, so a caller can leave the statement in place rather
 * than deleting it after one promote.
 */
export function planSay(state: AppState, id: string, text: string): Change[] {
  const index = indexOf(state, id);
  if (index < 0) return [];
  const before = state.sections[index].body;
  if (before.includes(text)) return [];
  return [
    {
      op: 'set',
      path: ['sections', index, 'body'],
      before,
      after: before ? `${before}\n\n${text}` : text,
    },
  ];
}

/** Set the narration line, and optionally mark the whole task finished. */
export function planNarrate(state: AppState, status: string, done: boolean): Change[] {
  return diffFields({ status: state.status, done: state.done }, { status, done }, []);
}

/** Throw the accumulated state away and start again from the seed. */
export function planReset(state: AppState): Change[] {
  const seed = initialState();
  const changes = planNarrate(state, seed.status, seed.done);
  const current = clone(state.sections as unknown as Json);
  const seeded = clone(seed.sections as unknown as Json);
  if (JSON.stringify(current) !== JSON.stringify(seeded)) {
    changes.push({ op: 'set', path: ['sections'], before: current, after: seeded });
  }
  return changes;
}

/** The one-line "what changed" a history row shows. */
export function describe(kind: string, id: string, changes: Change[]): string {
  if (kind === 'add') return `+ ${id}`;
  if (kind === 'remove') return `− ${id}`;
  if (kind === 'reset') return 'reset to the seed';
  if (kind === 'say') {
    const [change] = changes;
    const grew =
      change?.op === 'set' && typeof change.after === 'string' && typeof change.before === 'string'
        ? change.after.length - change.before.length
        : 0;
    return `${id} · +${grew} chars`;
  }
  const fields = changes.map((change) => String(change.path[change.path.length - 1]));
  return id ? `${id} · ${fields.join(', ')}` : fields.join(', ');
}
