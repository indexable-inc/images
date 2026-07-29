// Tests for the update surface.
//
// The one that matters is COMPOSITE IDEMPOTENCE. The update file re-runs top to
// bottom on every promote, so running it twice must leave the same state and the
// same history as running it once. Each mutator is idempotent alone, which is
// easy; the bug is in the SEQUENCES callers actually write. `add` then `say` on
// one section used to cost two entries per promote forever, because `add`
// upserted the caller's literal and reverted the appended body, which `say` then
// re-appended. Neither one's own test could see it.
//
// See history.test.ts on why imports carry the `.ts` extension.
import assert from 'node:assert/strict';
import { test } from 'node:test';

import { type Change, type Json, applyChange, clone } from './history.ts';
import {
  type AppState,
  type Section,
  initialState,
  planAdd,
  planNarrate,
  planRemove,
  planSay,
  planSet,
} from './plan.ts';

function blank(): AppState {
  return { status: 'start', done: false, sections: [] };
}

function section(id: string, body = ''): Section {
  return { id, title: id.toUpperCase(), loading: false, body };
}

/** Run a planner and apply what it planned, answering the changes it made. */
function run(state: AppState, plan: (s: AppState) => Change[]): Change[] {
  const changes = plan(state);
  for (const change of changes) applyChange(state as unknown as Json, change);
  return changes;
}

/**
 * An update file: the sequence of statements a caller leaves in place.
 *
 * Each field is DECLARED ONCE, which is the rule that makes a file idempotent
 * as a whole -- see the toggling test below for what happens otherwise.
 */
function updateFile(state: AppState): Change[] {
  return [
    ...run(state, (s) => planAdd(s, section('vrack-acl', 'Measured: 0.14 ms.'), 'top')),
    ...run(state, (s) => planSay(s, 'vrack-acl', 'Reproduced on a second host.')),
    ...run(state, (s) => planSet(s, 'vrack-acl', { loading: false })),
    ...run(state, (s) => planAdd(s, section('cve-scan', 'Gate is green.'))),
    ...run(state, (s) => planNarrate(s, 'two findings in', false)),
  ];
}

// --- the invariant ---------------------------------------------------------

test('re-running an unchanged update file changes nothing and records nothing', () => {
  const state = blank();

  const first = updateFile(state);
  assert.ok(first.length > 0, 'the first run does the work');
  const after = clone(state as unknown as Json);

  const second = updateFile(state);
  assert.deepEqual(second, [], 'the second run plans nothing at all');
  assert.deepEqual(state, after, 'and leaves the state byte-identical');

  // A third, because a bug that alternates would pass a single re-run.
  assert.deepEqual(updateFile(state), []);
});

test('add then say is idempotent together, not just separately', () => {
  // The exact regression: `add` must not re-apply its literal over a body that
  // `say` has since extended.
  const state = blank();
  run(state, (s) => planAdd(s, section('a', 'first')));
  run(state, (s) => planSay(s, 'a', 'second'));
  const body = state.sections[0].body;
  assert.equal(body, 'first\n\nsecond');

  assert.deepEqual(planAdd(state, section('a', 'first')), [], 'add plans nothing');
  assert.deepEqual(planSay(state, 'a', 'second'), [], 'say plans nothing');
  assert.equal(state.sections[0].body, body, 'the appended text is not reverted');
});

test('add does not overwrite a section that has since been edited', () => {
  const state = blank();
  run(state, (s) => planAdd(s, section('a', 'original')));
  run(state, (s) => planSet(s, 'a', { title: 'Renamed', loading: true }));
  assert.deepEqual(planAdd(state, section('a', 'original')), []);
  assert.equal(state.sections[0].title, 'Renamed');
  assert.equal(state.sections[0].loading, true);
});

// The residual sharp edge, pinned here rather than left for someone to discover
// on a live page. No per-call rule can fix it: each statement is applied
// eagerly, so a file that declares one field twice with different values really
// does change the state twice on every run, and the log is right to say so. The
// rule is therefore a rule about the FILE -- declare each field once -- and it
// is documented in AGENTS.md and in live.ts.
test('a file that sets one field to two values toggles on every re-run', () => {
  const state = blank();
  const flapping = (s: AppState): Change[] => [
    ...run(s, (x) => planNarrate(x, 'step one', false)),
    ...run(s, (x) => planNarrate(x, 'step two', false)),
  ];
  assert.equal(flapping(state).length, 2, 'the first run makes both transitions');
  assert.equal(flapping(state).length, 2, 'and so does every later run');
  assert.equal(state.status, 'step two', 'the end state is still correct');
});

// --- individual planners ---------------------------------------------------

test('a planner naming a section that is gone plans nothing', () => {
  const state = blank();
  assert.deepEqual(planSet(state, 'nope', { title: 'x' }), []);
  assert.deepEqual(planSay(state, 'nope', 'x'), []);
  assert.deepEqual(planRemove(state, 'nope'), []);
});

test('id is never patchable, because every entry addresses a section by it', () => {
  const state = blank();
  run(state, (s) => planAdd(s, section('a')));
  const changes = planSet(state, 'a', { id: 'b', title: 'T' });
  assert.deepEqual(
    changes.map((c) => c.path[c.path.length - 1]),
    ['title'],
  );
});

test('add places at top or bottom as asked', () => {
  const state = blank();
  run(state, (s) => planAdd(s, section('first')));
  run(state, (s) => planAdd(s, section('top'), 'top'));
  run(state, (s) => planAdd(s, section('bottom'), 'bottom'));
  assert.deepEqual(
    state.sections.map((s) => s.id),
    ['top', 'first', 'bottom'],
  );
});

test('remove carries the section away with it, so the change inverts', () => {
  const state = blank();
  run(state, (s) => planAdd(s, section('a', 'text')));
  const [change] = planRemove(state, 'a');
  assert.equal(change.op, 'delete');
  assert.deepEqual(change.op === 'delete' ? change.value : null, {
    id: 'a',
    title: 'A',
    loading: false,
    body: 'text',
  });
});

test('a planner never mutates the state it is given', () => {
  const state = blank();
  run(state, (s) => planAdd(s, section('a', 'text')));
  const before = clone(state as unknown as Json);
  planSet(state, 'a', { title: 'Other' });
  planSay(state, 'a', 'more');
  planRemove(state, 'a');
  planNarrate(state, 'different', true);
  assert.deepEqual(state, before, 'planning is pure; only `run` applies');
});

test('the seed is a fresh object every time', () => {
  const one = initialState();
  one.sections[0].body = 'mutated';
  assert.notEqual(initialState().sections[0].body, 'mutated');
});
