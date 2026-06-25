import assert from 'node:assert/strict';
import { describe, it } from 'node:test';

import { namespaceSessions } from '../src/lib/namespace-sessions.ts';
import { SCOPE_SEP } from '../src/lib/scope.ts';
import type { PaneRecord } from '../src/lib/types.ts';

function namespacePane(created_at: number): PaneRecord {
  return {
    kind: 'data',
    renderer: 'namespace',
    created_at,
    body: '[]',
  };
}

describe('namespaceSessions', () => {
  it('sorts namespace sessions by creation time', () => {
    const panes: Record<string, PaneRecord> = {
      [`z-oldest${SCOPE_SEP}namespace`]: namespacePane(100),
      [`a-newest${SCOPE_SEP}namespace`]: namespacePane(300),
      [`m-middle${SCOPE_SEP}namespace`]: namespacePane(200),
      [`early-non-namespace${SCOPE_SEP}run`]: { kind: 'exec', created_at: 1 },
    };

    assert.deepEqual(
      namespaceSessions(panes).map((s) => s.pane.scope),
      ['z-oldest', 'm-middle', 'a-newest'],
    );
  });
});
