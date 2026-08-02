// Tests for the storage reconciler.
//
// This is the ENG-11106 regression suite. That bug shipped through a green gate
// because the failure was a runtime `undefined` in a reader's browser, not a
// type error: `JSON.parse(raw) as AppState` asserts a shape the mirror does not
// have, and the cast is erased before anything can check it. Everything below
// feeds `reconcileState` a mirror written by a DIFFERENT shape and asserts the
// result is renderable.
//
// See history.test.ts on why imports carry the `.ts` extension.
import assert from 'node:assert/strict';
import { test } from 'node:test';

import { parseJson, reconcileHistory, reconcileState } from './shape.ts';
import type { AppState } from './store.svelte.ts';

function seed(): AppState {
  return {
    status: 'waiting',
    done: false,
    sections: [{ id: 'welcome', title: 'Welcome', loading: false, body: 'hi' }],
  };
}

// --- ENG-11106: a mirror from an older shape must not blank the page --------

test('a field the old shape never wrote comes back as the seed default', () => {
  // The mirror predates `done` existing at all.
  const stale = { status: 'mid-run', sections: [] };
  const state = reconcileState(stale, seed());
  assert.equal(state.done, false, 'filled from the seed, not left undefined');
  assert.equal(state.status, 'mid-run', 'and the reader keeps what they had');
});

test('a section missing a field renders instead of indexing into undefined', () => {
  // The exact ENG-11106 shape: a section written before `body` existed. The old
  // cast let this through and the component then read `.body` of undefined.
  const stale = { status: 'x', done: false, sections: [{ id: 'a', title: 'A' }] };
  const state = reconcileState(stale, seed());
  assert.deepEqual(state.sections, [{ id: 'a', title: 'A', loading: false, body: '' }]);
  for (const section of state.sections) {
    assert.equal(typeof section.body, 'string');
    assert.equal(typeof section.loading, 'boolean');
  }
});

test('adding a field to the store is absorbed, so an open page keeps its content', () => {
  // The property the whole design depends on: a reader who has had the page open
  // all afternoon keeps their sections AND gains the new field in one promote.
  const mirror = JSON.stringify({ status: 'live', done: false, sections: seed().sections });
  const state = reconcileState(parseJson(mirror), seed());
  assert.equal(state.sections.length, 1);
  assert.equal(state.sections[0].body, 'hi', 'content survived');
});

test('a field of the wrong type falls back rather than propagating', () => {
  const wrong = { status: 42, done: 'yes', sections: 'nope' };
  const state = reconcileState(wrong, seed());
  assert.deepEqual(state, seed());
});

test('an unrecognised field is dropped', () => {
  const extra = { status: 'x', done: true, sections: [], removedFeature: { a: 1 } };
  const state = reconcileState(extra, seed());
  assert.deepEqual(Object.keys(state).sort(), ['done', 'sections', 'status']);
});

test('one malformed section costs one section, not the page', () => {
  const mixed = {
    status: 'x',
    done: false,
    sections: [
      { id: 'good', title: 'Good', loading: false, body: 'b' },
      null,
      'garbage',
      { title: 'no id at all' },
      { id: 'also-good', title: 'Also', loading: false, body: 'c' },
    ],
  };
  const state = reconcileState(mixed, seed());
  assert.deepEqual(state.sections.map((s) => s.id), ['good', 'also-good']);
});

test('a section without an id is dropped rather than invented', () => {
  // An id is how a mutator addresses a section and how `{#each}` keys it. A
  // generated one would silently detach every history entry that names the real
  // one, so the section goes instead.
  const state = reconcileState({ status: 'x', done: false, sections: [{ title: 'T' }] }, seed());
  assert.deepEqual(state.sections, []);
});

test('only an unusable root falls back to the seed', () => {
  for (const raw of [null, undefined, 'string', 42, [], true]) {
    assert.deepEqual(reconcileState(raw, seed()), seed(), `root ${JSON.stringify(raw)}`);
  }
});

// --- the history mirror, which must fail independently ----------------------

test('an absent log is an empty history, never the content seed', () => {
  // Losing the log must cost the log and nothing else; that is why it has its
  // own storage key.
  const empty = { entries: [], head: '', open: false };
  assert.deepEqual(reconcileHistory(undefined), empty);
  assert.deepEqual(reconcileHistory('corrupt'), empty);
});

test('the panel stays open across a promote', () => {
  // Component-local state is remounted by a promote, so a reader watching an
  // agent work would lose the panel every seven seconds. It lives in the
  // persisted log instead.
  assert.equal(reconcileHistory({ entries: [], head: '', open: true }).open, true);
  assert.equal(reconcileHistory({ entries: [], head: '' }).open, false, 'defaults closed');
});

test('a well-formed entry survives the round trip', () => {
  const raw = {
    entries: [
      {
        seq: 7,
        ts: 1234,
        actor: { kind: 'human', label: 'andrew' },
        kind: 'say',
        target: 'welcome',
        label: 'welcome · +12 chars',
        changes: [{ op: 'set', path: ['sections', 0, 'body'], before: 'a', after: 'ab' }],
      },
    ],
    head: '{"status":"x"}',
  };
  const log = reconcileHistory(parseJson(JSON.stringify(raw)));
  assert.equal(log.entries.length, 1);
  assert.equal(log.entries[0].actor.label, 'andrew');
  assert.equal(log.entries[0].actor.kind, 'human');
  assert.equal(log.head, '{"status":"x"}');
});

test('an entry from a newer build, or a broken one, is dropped', () => {
  const raw = {
    entries: [
      { seq: 1, ts: 1, kind: 'teleport', target: '', label: '', changes: [] },
      { seq: 2, ts: 1, kind: 'set', target: '', label: '', changes: 'not an array' },
      { ts: 1, kind: 'set', target: '', label: '', changes: [] },
      { seq: 4, ts: 1, kind: 'set', target: '', label: '', changes: [] },
    ],
    head: '',
  };
  assert.deepEqual(reconcileHistory(raw).entries.map((e) => e.seq), [4]);
});

test('an entry with no actor is attributed to something rather than nobody', () => {
  const raw = { entries: [{ seq: 1, ts: 1, kind: 'set', target: '', label: '', changes: [] }] };
  const [only] = reconcileHistory(raw).entries;
  assert.equal(only.actor.kind, 'agent');
  assert.equal(only.actor.label, 'unknown');
});

test('parseJson answers undefined instead of throwing', () => {
  assert.equal(parseJson(null), undefined);
  assert.equal(parseJson('{'), undefined);
  assert.deepEqual(parseJson('{"a":1}'), { a: 1 });
});
