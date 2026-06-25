import assert from 'node:assert/strict';
import { describe, it } from 'node:test';

import { feedSessions } from '../src/lib/feed-sessions.ts';
import { SCOPE_SEP } from '../src/lib/scope.ts';
import type { PaneRecord } from '../src/lib/types.ts';

function sessionPane(title: string, created_at: number): PaneRecord {
  return {
    kind: 'data',
    renderer: 'session',
    title,
    created_at,
  };
}

describe('feedSessions', () => {
  it('sorts sessions by creation time', () => {
    const panes: Record<string, PaneRecord> = {
      [`late${SCOPE_SEP}session`]: sessionPane('A label sorts first', 300),
      [`early${SCOPE_SEP}session`]: sessionPane('Z label sorts last', 100),
      [`middle${SCOPE_SEP}run`]: { kind: 'exec', title: 'run', created_at: 200 },
    };

    assert.deepEqual(
      feedSessions(panes).map((s) => s.scope),
      ['early', 'middle', 'late'],
    );
  });

  it('uses scope as the stable tie-break', () => {
    const panes: Record<string, PaneRecord> = {
      [`bravo${SCOPE_SEP}session`]: sessionPane('Bravo', 100),
      [`alpha${SCOPE_SEP}session`]: sessionPane('Alpha', 100),
    };

    assert.deepEqual(
      feedSessions(panes).map((s) => s.scope),
      ['alpha', 'bravo'],
    );
  });
});
