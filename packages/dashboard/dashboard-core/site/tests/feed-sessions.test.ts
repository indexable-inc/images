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
  it('groups every scope and resolves its session label', () => {
    const panes: Record<string, PaneRecord> = {
      [`alpha${SCOPE_SEP}session`]: sessionPane('Alpha', 100),
      [`alpha${SCOPE_SEP}r1`]: { kind: 'exec', title: 'run', created_at: 200 },
      // A scope with no session-label pane falls back to a short scope.
      [`bravo${SCOPE_SEP}r1`]: { kind: 'exec', title: 'run', created_at: 150 },
    };

    assert.deepEqual(feedSessions(panes), [
      { scope: 'alpha', label: 'Alpha' },
      { scope: 'bravo', label: 'bravo' },
    ]);
  });

  it('orders by scope (a stable default; the sidebar re-orders by activity)', () => {
    const panes: Record<string, PaneRecord> = {
      [`charlie${SCOPE_SEP}session`]: sessionPane('C', 300),
      [`alpha${SCOPE_SEP}session`]: sessionPane('A', 100),
      [`bravo${SCOPE_SEP}session`]: sessionPane('B', 200),
    };

    assert.deepEqual(
      feedSessions(panes).map((s) => s.scope),
      ['alpha', 'bravo', 'charlie'],
    );
  });
});
