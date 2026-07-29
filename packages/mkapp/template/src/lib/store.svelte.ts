/// <reference types="vite/client" />
// The durable app store: the reactive shell around the document model in
// `plan.ts` and the version history in `history.ts`.
//
// State written here survives:
//  - hot reloads: dispose() stashes a snapshot in import.meta.hot.data and the
//    next module instance rehydrates from it, so a promote never resets the
//    page;
//  - full reloads: a debounced sessionStorage mirror rehydrates on boot, the
//    safety net for the reloads HMR cannot cover.
//
// Everything below the state is the update surface: named mutations that each
// record themselves. Mutate through them rather than assigning into `app`, and
// the page gains a browsable record of how it got here for free. This module
// holds no mutation LOGIC -- each function asks `plan.ts` what would change and
// hands the answer to `commit` -- so the update surface's behaviour is testable
// without a browser.

import {
  type Actor,
  type Change,
  type Entry,
  type HistoryState,
  type Json,
  applyChange,
  clone,
  stateAt,
} from './history.ts';
import {
  type AppState,
  type Section,
  describe,
  initialState,
  planAdd,
  planNarrate,
  planRemove,
  planReset,
  planSay,
  planSet,
} from './plan.ts';
import { parseJson, reconcileHistory, reconcileState } from './shape.ts';

export type { AppState, Section };

// Two keys, not one. The content is what the reader came for and the history is
// commentary on it, so they must be able to fail independently: a log that grows
// past the storage quota has to cost the log, never the page.
const STATE_KEY = 'mkapp:state';
const HISTORY_KEY = 'mkapp:history';
const PERSIST_DEBOUNCE_MS = 250;

// Two caps, because entries vary hugely in size: a `narrate` is a few dozen
// bytes and a `say` on a long body carries that body twice. A count alone lets a
// handful of large entries fill the quota; a budget alone lets thousands of tiny
// ones make the panel unreadable.
const MAX_ENTRIES = 400;
const MAX_HISTORY_BYTES = 512_000;

// The state and the log are separate objects under separate keys, which makes
// ADDING the history a purely additive change: an open page's content mirror is
// untouched by it, so nobody's page blanks on the promote that ships this.
// `shape.ts` then hardens every future shape change.
function rehydrate(): { state: AppState; history: HistoryState } {
  const handoff = import.meta.hot?.data.store as
    | { state: AppState; history: HistoryState }
    | undefined;
  if (handoff) return handoff;
  const seed = initialState();
  let state = seed;
  let log: HistoryState = { entries: [], head: '', open: false };
  try {
    state = reconcileState(parseJson(sessionStorage.getItem(STATE_KEY)), seed);
    log = reconcileHistory(parseJson(sessionStorage.getItem(HISTORY_KEY)));
  } catch {
    // Unreadable storage means a fresh state, never a broken boot.
  }
  if (log.entries.length === 0) {
    // The floor of the history: the page as this reader first received it, so
    // time travel has somewhere to end.
    log.entries = [
      {
        seq: 0,
        ts: Date.now(),
        actor: { kind: 'agent', label: 'mkapp' },
        kind: 'seed',
        target: '',
        label: 'page loaded',
        changes: [{ op: 'set', path: [], after: clone(state as unknown as Json) }],
      },
    ];
    log.head = JSON.stringify(state);
  }
  return { state, history: log };
}

const restored = rehydrate();

export const app = $state<AppState>(restored.state);
export const history = $state<HistoryState>(restored.history);

/** Which entry the page is being viewed at; null means live. */
export const viewing = $state<{ seq: number | null }>({ seq: null });

let nextSeq = history.entries.reduce((high, entry) => Math.max(high, entry.seq + 1), 1);

if (import.meta.hot) {
  import.meta.hot.dispose((data) => {
    data.store = { state: $state.snapshot(app), history: $state.snapshot(history) };
  });
}

// ---------------------------------------------------------------------------
// Recording
// ---------------------------------------------------------------------------

/**
 * Who the following statements are attributed to.
 *
 * There is one writer -- the serving agent -- but it relays work from many
 * subagents, so attribution is wanted even with no concurrency to resolve. It is
 * ambient rather than a per-call argument because the update file is a flat
 * script and reads as a block per subagent.
 */
let actor: Actor = { kind: 'agent', label: 'agent' };

/** Attribute every later statement to `label`. */
export function by(label: string, kind: Actor['kind'] = 'agent'): void {
  actor = { kind, label };
}

/** The live state as plain JSON, which is what every planner takes. */
function snapshot(): AppState {
  return $state.snapshot(app) as AppState;
}

function trim(): void {
  while (history.entries.length > MAX_ENTRIES) history.entries.shift();
  while (history.entries.length > 1 && JSON.stringify(history.entries).length > MAX_HISTORY_BYTES) {
    history.entries.shift();
  }
}

/**
 * Apply a planned mutation and record it, and only if it changes something.
 *
 * THE LOAD-BEARING INVARIANT: the log records state TRANSITIONS, not calls. The
 * update file re-runs top to bottom on every promote and its statements are
 * idempotent by design, so a log that appended per call would grow by the whole
 * file every seven seconds and read as hundreds of rows saying nothing happened.
 * Every planner returns an empty array when nothing differs, and that lands
 * here.
 */
function commit(kind: Entry['kind'], target: string, changes: Change[]): void {
  if (changes.length === 0) return;
  // Applied through the SAME `applyChange` that time travel inverts. One code
  // path, so a bug in it breaks the live page immediately and loudly rather than
  // only the undo direction, and only for whoever scrubs the history. `app` is a
  // rune proxy: descending it and assigning is what makes this reactive.
  for (const change of changes) applyChange(app as unknown as Json, change);
  history.entries.push({
    seq: nextSeq++,
    ts: Date.now(),
    actor: { ...actor },
    kind,
    target,
    label: describe(kind, target, changes),
    changes,
  });
  trim();
  history.head = JSON.stringify(app);
  // A change lands on the live page, so stop showing the past.
  viewing.seq = null;
}

// ---------------------------------------------------------------------------
// The update API
//
// Every function is idempotent, and so is every SEQUENCE of them: running the
// update file twice looks exactly like running it once, in the state and in the
// history. `plan.test.ts` pins that.
// ---------------------------------------------------------------------------

/** The section named `id`, or undefined. */
export function get(id: string): Section | undefined {
  return app.sections.find((section) => section.id === id);
}

/** Change part of one section, leaving the rest of the page untouched. */
export function set(id: string, patch: Partial<Section>): void {
  commit('set', id, planSet(snapshot(), id, patch));
}

/**
 * File a section, if it is not already there.
 *
 * Insert-if-absent rather than upsert; see `planAdd` for why that is what makes
 * it safe to leave in the update file. Use `set` to change one that exists.
 */
export function add(section: Section, where: 'top' | 'bottom' = 'bottom'): void {
  commit('add', section.id, planAdd(snapshot(), section, where));
}

/** Drop a section. An id that is not there is not an error. */
export function remove(id: string): void {
  commit('remove', id, planRemove(snapshot(), id));
}

/** Append a paragraph to a section's body, once. */
export function say(id: string, text: string): void {
  commit('say', id, planSay(snapshot(), id, text));
}

/** Set the narration line, and optionally mark the whole task finished. */
export function narrate(status: string, done = false): void {
  commit('narrate', '', planNarrate(snapshot(), status, done));
}

/**
 * Throw the reader's accumulated state away and start again from the seed.
 *
 * The escape hatch for the sharp edge of an imperative update model: DELETING a
 * statement does not undo it, because the change is already in the reader's
 * store. To undo, push the inverse -- or call this once and remove it
 * afterwards, since leaving it in place would wipe every later statement on
 * every promote.
 *
 * Recorded like anything else, so a reader can still scrub back past it.
 */
export function reset(): void {
  commit('reset', '', planReset(snapshot()));
}

// ---------------------------------------------------------------------------
// Viewing the past
// ---------------------------------------------------------------------------

/** Show or hide the history panel. */
export function toggleHistory(open = !history.open): void {
  history.open = open;
}

/** Show the page as it stood just after `seq`. `null` returns to live. */
export function viewAt(seq: number | null): void {
  viewing.seq = seq;
}

/**
 * The state to render: the live one, or a reconstruction of a past one.
 *
 * Call this inside a `$derived` -- it reads `viewing`, `app` and the log, so it
 * re-runs when any of them changes.
 */
export function viewState(): AppState {
  if (viewing.seq === null) return app;
  const index = history.entries.findIndex((entry) => entry.seq === viewing.seq);
  if (index < 0) return app;
  return stateAt(
    snapshot() as unknown as Json,
    $state.snapshot(history.entries) as unknown as Entry[],
    index,
  ) as unknown as AppState;
}

// ---------------------------------------------------------------------------
// Persistence, and the guard that keeps the log honest
// ---------------------------------------------------------------------------

/**
 * Notice a change that reached the state without going through a mutator.
 *
 * A component assigning `app.sections[0].title` directly bypasses the log, and a
 * history that quietly omits it is worse than no history: it is a record that
 * looks complete and is not. `history.head` is the serialization the log
 * accounts for, so any mismatch is both the alarm and the exact before-image of
 * what was missed -- which keeps backwards reconstruction correct ACROSS the gap
 * rather than merely flagging it. It also covers the two storage keys
 * disagreeing after a reload, since `head` is persisted with the log.
 */
function reconcileHead(content: string): void {
  if (content === history.head) return;
  const before = parseJson(history.head);
  const after = parseJson(content);
  history.head = content;
  if (before === undefined || after === undefined) return;
  history.entries.push({
    seq: nextSeq++,
    ts: Date.now(),
    actor: { kind: 'human', label: 'browser' },
    kind: 'external',
    target: '',
    label: 'changed outside the update surface',
    changes: [{ op: 'set', path: [], before: before as Json, after: after as Json }],
  });
  trim();
}

let stateTimer: ReturnType<typeof setTimeout> | undefined;
let historyTimer: ReturnType<typeof setTimeout> | undefined;

// Two effects rather than one, so the guard can write to the log without the
// effect that reads the log re-triggering it. $effect.root gives module scope an
// effect context; JSON.stringify reads every property of the state proxy, so any
// deep change reschedules the write.
$effect.root(() => {
  $effect(() => {
    const content = JSON.stringify(app);
    reconcileHead(content);
    clearTimeout(stateTimer);
    stateTimer = setTimeout(() => {
      try {
        sessionStorage.setItem(STATE_KEY, content);
      } catch {
        // Storage full or blocked: the HMR handoff still preserves state.
      }
    }, PERSIST_DEBOUNCE_MS);
  });

  $effect(() => {
    const log = JSON.stringify(history);
    clearTimeout(historyTimer);
    historyTimer = setTimeout(() => {
      try {
        sessionStorage.setItem(HISTORY_KEY, log);
      } catch {
        // The log is the first thing to go when the quota runs out, and losing
        // it must not cost the page: the content is written by its own effect,
        // under its own key, and is already safe by the time this throws.
      }
    }, PERSIST_DEBOUNCE_MS);
  });
});
