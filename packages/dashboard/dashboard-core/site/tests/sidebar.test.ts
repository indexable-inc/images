import assert from 'node:assert/strict';
import { describe, it } from 'node:test';

import { buildSidebar, flattenVisible } from '../src/lib/sidebar.ts';
import { SCOPE_SEP } from '../src/lib/scope.ts';
import type { PaneRecord } from '../src/lib/types.ts';

function sessionPane(title: string, created_at: number): PaneRecord {
  return { kind: 'data', renderer: 'session', title, created_at };
}
function run(title: string, created_at: number): PaneRecord {
  return { kind: 'exec', title, created_at, ok: true };
}

const S = 'sessA';
const T = 'sessB';

const panes: Record<string, PaneRecord> = {
  [`${S}${SCOPE_SEP}__session__`]: sessionPane('session A', 100),
  [`${S}${SCOPE_SEP}r1`]: run('first run', 110),
  [`${S}${SCOPE_SEP}r2`]: run('second run', 130),
  // A rich-output attachment must not appear as its own run.
  [`${S}${SCOPE_SEP}r2/out`]: { kind: 'html', title: 'out', created_at: 131 },
  // A namespace pane belongs to the rail, never the run list.
  [`${S}${SCOPE_SEP}ns`]: { kind: 'data', renderer: 'namespace', body: '[]', created_at: 105 },
  // A terminal is a resource.
  [`${S}${SCOPE_SEP}resource/term`]: { kind: 'terminal', title: 'shell', created_at: 120, alive: true },
  // A second session with one run.
  [`${T}${SCOPE_SEP}__session__`]: sessionPane('session B', 200),
  [`${T}${SCOPE_SEP}r1`]: run('only run', 210),
};

describe('buildSidebar', () => {
  const model = buildSidebar(panes, []);

  it('groups runs under their session, excluding non-run panes', () => {
    const a = model.sessions.find((s) => s.scope === S);
    assert.ok(a);
    assert.equal(a.label, 'session A');
    // r1 and r2 only — not the /out attachment, the namespace, or the terminal.
    assert.deepEqual(
      a.runs.map((r) => r.pane.title),
      ['first run', 'second run'],
    );
  });

  it('orders runs oldest-first within a session (a log growing downward)', () => {
    const a = model.sessions.find((s) => s.scope === S);
    assert.deepEqual(a?.runs.map((r) => r.pane.created_at), [110, 130]);
  });

  it('collects terminals and resource/* panes as resources', () => {
    assert.deepEqual(
      model.resources.map((r) => r.pane.title),
      ['shell'],
    );
  });

  it('counts every run across sessions', () => {
    assert.equal(model.runCount, 3);
    assert.equal(model.sessions.length, 2);
  });
});

describe('flattenVisible', () => {
  const model = buildSidebar(panes, [
    { id: 'rec1', started_ms: 1, updated_ms: 2, bytes: 10 },
  ]);
  const allOpen = () => true;

  it('walks runs, resources, then recordings in render order', () => {
    const rows = flattenVisible(model, allOpen);
    assert.deepEqual(rows.map((r) => r.selection), [
      { kind: 'run', key: `${S}${SCOPE_SEP}r1` },
      { kind: 'run', key: `${S}${SCOPE_SEP}r2` },
      { kind: 'run', key: `${T}${SCOPE_SEP}r1` },
      { kind: 'resource', key: `${S}${SCOPE_SEP}resource/term` },
      { kind: 'recording', id: 'rec1' },
    ]);
  });

  it('hides a folded session and folded sections', () => {
    const rows = flattenVisible(model, (k) => k !== 'sess:' + S && k !== 'recordings');
    assert.deepEqual(rows.map((r) => r.selection), [
      { kind: 'run', key: `${T}${SCOPE_SEP}r1` },
      { kind: 'resource', key: `${S}${SCOPE_SEP}resource/term` },
    ]);
  });

  it('hides everything under a folded top section', () => {
    const rows = flattenVisible(model, (k) => k !== 'sessions');
    assert.ok(rows.every((r) => r.selection.kind !== 'run'));
  });
});
