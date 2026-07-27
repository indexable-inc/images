import assert from 'node:assert/strict';
import { describe, it } from 'node:test';

import { changeId, frontierAt, markOf, sortMarks, type Mark } from '../src/lib/frontier.ts';
import { peerBadge, readPeers, shortPeer } from '../src/lib/peers.ts';
import { formatValue, rootPath, summarize } from '../src/lib/edits.ts';

function mark(peer: string, start: number, ts: number, lamport: number, length = 1): Mark {
  return markOf(peer, { counter: start, length, timestamp: ts, lamport });
}

describe('frontierAt', () => {
  it('is empty before any change', () => {
    assert.deepEqual(frontierAt([], 1000), []);
    assert.deepEqual(frontierAt([mark('1', 0, 5000, 0)], 1000), []);
  });

  it('names one peer when only one peer has written', () => {
    const marks = sortMarks([mark('1', 0, 1000, 0), mark('1', 1, 3000, 1)]);
    assert.deepEqual(frontierAt(marks, 3000), [{ peer: '1', counter: 1 }]);
    assert.deepEqual(frontierAt(marks, 2000), [{ peer: '1', counter: 0 }]);
  });

  // The bug this function exists to fix. Two viewers branch off one snapshot and
  // answer concurrently; naming only the LAST change checks out a version where
  // the other answer was never made, and it silently disappears from the replay.
  it('keeps every concurrent peer, not just the newest change', () => {
    const marks = sortMarks([
      mark('9', 0, 1000, 0), // the aggregator's base
      mark('1', 0, 2000, 1), // viewer A answers
      mark('2', 0, 3000, 1), // viewer B answers, concurrently
    ]);
    const frontier = frontierAt(marks, 3000);
    assert.equal(frontier.length, 3, 'a frontier is a set, one entry per peer');
    assert.deepEqual(
      [...frontier].sort((a, b) => a.peer.localeCompare(b.peer)),
      [
        { peer: '1', counter: 0 },
        { peer: '2', counter: 0 },
        { peer: '9', counter: 0 },
      ],
    );
  });

  it('takes each peer\'s LAST op at or before the moment', () => {
    const marks = sortMarks([
      mark('1', 0, 1000, 0),
      mark('1', 5, 2000, 2, 4), // ops 5..8
      mark('2', 0, 1500, 1),
      mark('1', 20, 9000, 9), // after the cut
    ]);
    assert.deepEqual(
      [...frontierAt(marks, 3000)].sort((a, b) => a.peer.localeCompare(b.peer)),
      [
        { peer: '1', counter: 8 },
        { peer: '2', counter: 0 },
      ],
    );
  });
});

describe('markOf', () => {
  it('records the span and the last counter', () => {
    const m = markOf('42', { counter: 10, length: 4, timestamp: 7, lamport: 3 });
    assert.deepEqual(m, { peer: '42', start: 10, length: 4, counter: 13, ts: 7, lamport: 3 });
    assert.equal(changeId(m.peer, m.start), '10@42');
  });
});

describe('peer attribution', () => {
  it('reads kind and label out of __peers', () => {
    const peers = readPeers({ '12': { kind: 'agent', label: 'splicer' } });
    assert.deepEqual(peers['12'], { kind: 'agent', label: 'splicer' });
  });

  it('skips entries with nothing usable rather than inventing one', () => {
    assert.deepEqual(readPeers({ '12': {}, '13': 'nope', '14': null }), {});
    assert.deepEqual(readPeers(null), {});
  });

  it('keeps a partial entry, filling only the missing half', () => {
    const peers = readPeers({ '4821': { label: 'hub' }, '99': { kind: 'agent' } });
    assert.deepEqual(peers['4821'], { kind: 'unknown', label: 'hub' });
    assert.deepEqual(peers['99'], { kind: 'agent', label: 'peer 99' });
  });

  // Degradation is the point: __peers is written by other processes and is always
  // allowed to be incomplete, so an unregistered peer still gets an identity.
  it('names an unregistered peer from its peer id', () => {
    const badge = peerBadge({}, '1234567890', '55');
    assert.deepEqual(badge, { kind: 'unknown', label: 'peer 7890', you: false, anonymous: true });
  });

  it('calls this browser "you" even when it never registered', () => {
    const badge = peerBadge({}, '55', '55');
    assert.deepEqual(badge, { kind: 'human', label: 'you', you: true, anonymous: true });
  });

  it('prefers the registered label but still flags it as you', () => {
    const badge = peerBadge({ '55': { kind: 'human', label: 'andrew' } }, '55', '55');
    assert.deepEqual(badge, { kind: 'human', label: 'andrew', you: true, anonymous: false });
  });

  it('shortens a peer id without going through a number', () => {
    // 2^63 does not survive Number(); slicing characters does.
    assert.equal(shortPeer('9223372036854775807'), '5807');
    assert.equal(shortPeer('42'), '42');
  });
});

const NO_PATHS = () => undefined;

function change(ops: { container: string; content: unknown }[]) {
  return {
    id: '0@1' as const,
    timestamp: 0,
    deps: [],
    lamport: 0,
    msg: null,
    ops: ops.map((op, counter) => ({ ...op, counter })),
  } as unknown as Parameters<typeof summarize>[0];
}

describe('summarize', () => {
  it('reads a root container path off its id when the resolver has nothing', () => {
    assert.deepEqual(rootPath('cid:root-inputs:Map'), ['inputs']);
    assert.equal(rootPath('cid:12@4821:Text'), null);
  });

  it('leads with the answer someone gave', () => {
    const out = summarize(
      change([
        { container: 'cid:root-inputs:Map', content: { type: 'insert', key: 'verdict', value: 'splice' } },
      ]),
      NO_PATHS,
    );
    assert.equal(out.what, 'verdict = splice');
    assert.equal(out.where, 'inputs');
    assert.equal(out.target, null, 'a key that is not a pane key has nowhere to scroll');
    assert.deepEqual(out.inputs, ['verdict']);
  });

  // The default input key IS the asking pane's doc key, whose 0x1f separator is
  // invisible; unread it so the row says "r7", not "sessr7".
  it('reads a pane-keyed answer back as the pane, and points at it', () => {
    const key = `sess${String.fromCharCode(0x1f)}r7`;
    const out = summarize(
      change([
        { container: 'cid:root-inputs:Map', content: { type: 'insert', key, value: 'approve' } },
      ]),
      NO_PATHS,
    );
    assert.equal(out.what, 'r7 = approve');
    assert.equal(out.where, 'r7 · answer');
    assert.deepEqual(out.target, { paneKey: key, field: '' });
  });

  it('reports the SIZE of a text edit, never its position', () => {
    const out = summarize(
      change([
        { container: 'cid:1@7:Text', content: { type: 'insert', pos: 4096, text: 'hello world' } },
      ]),
      () => ['panes', 'sessr7', 'stdout'],
    );
    // 4096 is an entity index, not a character offset, so it must not appear.
    assert.equal(out.what, '+11');
    assert.equal(out.where, 'r7 · stdout');
    assert.deepEqual(out.target, { paneKey: 'sessr7', field: 'stdout' });
    assert.doesNotMatch(out.what, /4096/);
  });

  it('names a pane appearing', () => {
    const out = summarize(
      change([
        {
          container: 'cid:root-panes:Map',
          content: { type: 'insert', key: 'sessr9', value: '🦜:cid:0@7:Map' },
        },
      ]),
      NO_PATHS,
    );
    assert.equal(out.what, '+ r9');
    assert.equal(out.where, 'panes');
  });

  it('folds a peer registration into "joined"', () => {
    const out = summarize(
      change([
        { container: 'cid:root-__peers:Map', content: { type: 'insert', key: '55', value: {} } },
      ]),
      NO_PATHS,
    );
    assert.equal(out.what, 'joined');
    assert.equal(out.where, 'peers');
  });
});

describe('formatValue', () => {
  it('renders a nested container as its type, not its id', () => {
    assert.equal(formatValue('🦜:cid:0@7:Text'), '«text»');
  });

  it('truncates long strings and passes scalars through', () => {
    assert.equal(formatValue(12), '12');
    assert.equal(formatValue(true), 'true');
    assert.equal(formatValue(null), 'null');
    assert.equal(formatValue('x'.repeat(40)), `${'x'.repeat(27)}…`);
  });
});
