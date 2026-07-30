// Tests for the pure history core.
//
// Run by `npm run check:staging`, so a bug here cannot reach a promote. That
// matters more than usual: the class of bug this file guards against (ENG-11106)
// fails at runtime in a reader's browser, where every other signal -- the gate,
// the promote, the dev server -- still reads green.
//
// Imports carry the `.ts` extension because `node --test` strips types without
// rewriting specifiers, so an extensionless relative import would not resolve.
// Vite and svelte-check accept both.
import assert from 'node:assert/strict';
import { test } from 'node:test';

import {
  type Change,
  type Entry,
  type Json,
  applyChange,
  clone,
  diffFields,
  invert,
  shortAge,
  stateAt,
} from './history.ts';

function entry(seq: number, changes: Change[]): Entry {
  return {
    seq,
    ts: 1_000 * seq,
    actor: { kind: 'agent', label: 'test' },
    kind: 'set',
    target: '',
    label: `entry ${seq}`,
    changes,
  };
}

// --- the central invariant -------------------------------------------------
// The update file re-runs on every promote with identical, idempotent
// statements. If a no-op patch produced an entry, the history would grow by the
// whole file every seven seconds.

test('a patch that changes nothing produces no changes', () => {
  const current = { title: 'Welcome', loading: false, body: 'hello' };
  assert.deepEqual(diffFields(current, { title: 'Welcome', loading: false }, ['s', 0]), []);
});

test('a patch produces exactly the fields that differ', () => {
  const current = { title: 'Welcome', loading: false, body: 'hello' };
  const changes = diffFields(current, { title: 'Welcome', loading: true }, ['s', 0]);
  assert.deepEqual(changes, [
    { op: 'set', path: ['s', 0, 'loading'], before: false, after: true },
  ]);
});

test('an undefined value means "not mentioned", not "set to undefined"', () => {
  const current = { title: 'Welcome', body: 'hello' };
  assert.deepEqual(diffFields(current, { title: undefined }, []), []);
});

test('skipped keys are never diffed', () => {
  const current = { id: 'welcome', title: 'Welcome' };
  const changes = diffFields(current, { id: 'renamed', title: 'New' }, [], ['id']);
  assert.deepEqual(changes.map((c) => c.path), [['title']]);
});

// --- inversion -------------------------------------------------------------

test('every op inverts to its own undo', () => {
  const set: Change = { op: 'set', path: ['a'], before: 1, after: 2 };
  assert.deepEqual(invert(set), { op: 'set', path: ['a'], before: 2, after: 1 });

  const insert: Change = { op: 'insert', path: ['xs'], index: 1, value: 'b' };
  assert.deepEqual(invert(insert), { op: 'delete', path: ['xs'], index: 1, value: 'b' });

  const del: Change = { op: 'delete', path: ['xs'], index: 0, value: 'a' };
  assert.deepEqual(invert(del), { op: 'insert', path: ['xs'], index: 0, value: 'a' });
});

test('inverting twice is the identity', () => {
  for (const change of [
    { op: 'set', path: ['a', 0, 'b'], before: 'x', after: 'y' },
    { op: 'insert', path: ['xs'], index: 2, value: { k: 1 } },
    { op: 'delete', path: ['xs'], index: 2, value: { k: 1 } },
  ] satisfies Change[]) {
    assert.deepEqual(invert(invert(change)), change);
  }
});

test('an absent before-image inverts to removing the key', () => {
  const added: Change = { op: 'set', path: ['a'], after: 1 };
  const undo = invert(added);
  const root: Json = { a: 1 };
  applyChange(root, undo);
  assert.deepEqual(root, {}, 'the key is deleted, not set to undefined');
  assert.ok(!('a' in root));
});

// --- applying --------------------------------------------------------------

test('applyChange sets, inserts and deletes', () => {
  const root: Json = { title: 'a', xs: ['p', 'q'] };
  applyChange(root, { op: 'set', path: ['title'], before: 'a', after: 'b' });
  applyChange(root, { op: 'insert', path: ['xs'], index: 1, value: 'mid' });
  applyChange(root, { op: 'delete', path: ['xs'], index: 0, value: 'p' });
  assert.deepEqual(root, { title: 'b', xs: ['mid', 'q'] });
});

test('a path that no longer resolves is skipped, not thrown', () => {
  const root: Json = { xs: [] };
  assert.doesNotThrow(() => {
    applyChange(root, { op: 'set', path: ['gone', 'deeper', 'k'], after: 1 });
    applyChange(root, { op: 'insert', path: ['missing'], index: 0, value: 1 });
    applyChange(root, { op: 'set', path: ['xs', 9, 'k'], after: 1 });
  });
  assert.deepEqual(root, { xs: [] }, 'a skipped change leaves the state alone');
});

test('an empty path replaces the whole root', () => {
  const replaced = applyChange({ a: 1 }, { op: 'set', path: [], before: { a: 1 }, after: { b: 2 } });
  assert.deepEqual(replaced, { b: 2 });
});

// A version history whose own record changes underneath it is worse than none.
// This is a real bug the suite caught: `applyChange` used to splice
// `change.value` into the state by reference, so the next edit to that section
// reached back through the alias and rewrote the log's before-image. The
// history then quietly agreed with the present and reconstruction was wrong for
// every state before it.
test('the log never shares structure with the state it is applied to', () => {
  const insert: Change = { op: 'insert', path: ['xs'], index: 0, value: { body: 'original' } };
  const root: Json = { xs: [] };
  applyChange(root, insert);

  const inserted = (root as { xs: { body: string }[] }).xs[0];
  inserted.body = 'mutated later';

  assert.deepEqual(insert.value, { body: 'original' }, 'the recorded change is untouched');
});

test('a set writes a copy, so the state cannot reach back into the log', () => {
  const change: Change = { op: 'set', path: ['meta'], before: null, after: { tag: 'a' } };
  const root: Json = { meta: null };
  applyChange(root, change);
  (root as { meta: { tag: string } }).meta.tag = 'b';
  assert.deepEqual(change.after, { tag: 'a' });
});

// --- reconstruction --------------------------------------------------------

test('stateAt at the newest entry is the current state', () => {
  const current = { n: 3 };
  const entries = [entry(0, []), entry(1, [{ op: 'set', path: ['n'], before: 2, after: 3 }])];
  assert.deepEqual(stateAt(current, entries, 1), current);
});

test('stateAt does not mutate the state it is given', () => {
  const current = { xs: ['a', 'b'] };
  const entries = [entry(0, []), entry(1, [{ op: 'insert', path: ['xs'], index: 1, value: 'b' }])];
  stateAt(current, entries, 0);
  assert.deepEqual(current, { xs: ['a', 'b'] });
});

// The real property: drive the SAME functions the store drives, remember what
// the state looked like after each step, then check every reconstruction against
// what was actually observed. This is what "browsable version history" means,
// and it is the one test that would catch an off-by-one in the backwards walk.
test('every past state reconstructs exactly as it was observed', () => {
  let live: Json = { status: 'start', done: false, sections: [] };
  const entries: Entry[] = [];
  const observed: Json[] = [];

  const step = (changes: Change[]): void => {
    for (const change of changes) live = applyChange(live, change);
    entries.push(entry(entries.length, changes));
    observed.push(clone(live));
  };

  step([{ op: 'set', path: ['status'], before: 'start', after: 'working' }]);
  step([{ op: 'insert', path: ['sections'], index: 0, value: { id: 'a', body: '' } }]);
  step([{ op: 'insert', path: ['sections'], index: 1, value: { id: 'b', body: 'bee' } }]);
  step([{ op: 'set', path: ['sections', 0, 'body'], before: '', after: 'first' }]);
  step([{ op: 'insert', path: ['sections'], index: 0, value: { id: 'c', body: 'sea' } }]);
  // A deletion in the middle: the case where a naive index-based walk goes wrong.
  step([{ op: 'delete', path: ['sections'], index: 1, value: { id: 'a', body: 'first' } }]);
  step([
    { op: 'set', path: ['status'], before: 'working', after: 'done' },
    { op: 'set', path: ['done'], before: false, after: true },
  ]);

  for (let i = 0; i < entries.length; i++) {
    assert.deepEqual(stateAt(live, entries, i), observed[i], `state after entry ${i}`);
  }
});

test('a deleted section comes back when scrubbed past its removal', () => {
  const before: Json = { sections: [{ id: 'gone', body: 'text' }] };
  const changes: Change[] = [
    { op: 'delete', path: ['sections'], index: 0, value: { id: 'gone', body: 'text' } },
  ];
  const live = applyChange(clone(before), changes[0]);
  const entries = [entry(0, []), entry(1, changes)];
  assert.deepEqual(stateAt(live, entries, 1), { sections: [] });
  assert.deepEqual(stateAt(live, entries, 0), before);
});

// --- presentation ----------------------------------------------------------

test('shortAge reads as a bare relative age', () => {
  const now = 10_000_000;
  assert.equal(shortAge(now, now), 'now');
  assert.equal(shortAge(now - 30_000, now), '30s');
  assert.equal(shortAge(now - 4 * 60_000, now), '4m');
  assert.equal(shortAge(now - 3 * 3_600_000, now), '3h');
  assert.equal(shortAge(now + 5_000, now), 'now', 'a clock skew never reads as negative');
});
