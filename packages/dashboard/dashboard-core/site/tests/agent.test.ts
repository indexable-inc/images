import assert from 'node:assert/strict';
import { describe, it } from 'node:test';

import { inputKey, parseTranscript, transcriptKeyFor } from '../src/lib/agent.ts';
import { isAgentTerminal, isTranscriptPane } from '../src/lib/run.ts';
import { SCOPE_SEP } from '../src/lib/scope.ts';
import type { PaneRecord } from '../src/lib/types.ts';

describe('parseTranscript', () => {
  it('reads the producer wire shape: role, text, tool, usage, skipped', () => {
    const body = JSON.stringify({
      agent: 'claude',
      entries: [
        { role: 'user', text: 'do the thing', ts: '2026-08-05T00:00:01Z' },
        { role: 'assistant', text: '', tool: 'Bash' },
        { role: 'assistant', text: 'done', usage: { input_tokens: 10, output_tokens: 20 } },
      ],
      skipped: 2,
    });
    const t = parseTranscript(body);
    assert.equal(t.entries.length, 3);
    assert.deepEqual(t.entries[0], { role: 'user', text: 'do the thing', tool: undefined, usage: undefined, ts: '2026-08-05T00:00:01Z' });
    assert.equal(t.entries[1].tool, 'Bash');
    assert.deepEqual(t.entries[2].usage, { input_tokens: 10, output_tokens: 20 });
    assert.equal(t.skipped, 2);
  });

  it('degrades malformed bodies to an empty transcript instead of throwing', () => {
    assert.deepEqual(parseTranscript(undefined), { entries: [], skipped: 0 });
    assert.deepEqual(parseTranscript('not json'), { entries: [], skipped: 0 });
    assert.deepEqual(parseTranscript('[1,2,3]').entries, []);
    // A row missing its role is dropped; well-formed neighbours survive.
    const mixed = parseTranscript(JSON.stringify({ entries: [{ text: 'no role' }, { role: 'user', text: 'ok' }], skipped: 0 }));
    assert.deepEqual(mixed.entries.map((e) => e.text), ['ok']);
    // Malformed usage is dropped, the row kept.
    const badUsage = parseTranscript(JSON.stringify({ entries: [{ role: 'assistant', text: 'hi', usage: { input_tokens: 'x' } }], skipped: 0 }));
    assert.equal(badUsage.entries[0].usage, undefined);
  });
});

describe('agent pane classification', () => {
  const term: PaneRecord = { kind: 'terminal', agent: 'claude', status: 'working' };
  const transcript: PaneRecord = { kind: 'data', renderer: 'transcript', parent: 'a1' };

  it('an agent terminal is its agent label, a bare terminal is not', () => {
    assert.ok(isAgentTerminal(term));
    assert.ok(!isAgentTerminal({ kind: 'terminal' }));
    assert.ok(!isAgentTerminal({ kind: 'terminal', agent: '' }));
    assert.ok(!isAgentTerminal(transcript));
  });

  it('a transcript pane is data + renderer:transcript', () => {
    assert.ok(isTranscriptPane(transcript));
    assert.ok(!isTranscriptPane({ kind: 'data', renderer: 'namespace' }));
    assert.ok(!isTranscriptPane(term));
  });
});

describe('transcriptKeyFor', () => {
  const scope = 'prod';
  const panes: Record<string, PaneRecord> = {
    [`${scope}${SCOPE_SEP}a1`]: { kind: 'terminal', agent: 'claude' },
    [`${scope}${SCOPE_SEP}a1-transcript`]: { kind: 'data', renderer: 'transcript', parent: 'a1' },
    // A transcript in ANOTHER scope with the same parent id must not match.
    [`other${SCOPE_SEP}a1-transcript`]: { kind: 'data', renderer: 'transcript', parent: 'a1' },
  };

  it('resolves the companion by scope + parent, not by key convention', () => {
    assert.equal(transcriptKeyFor(panes, `${scope}${SCOPE_SEP}a1`), `${scope}${SCOPE_SEP}a1-transcript`);
  });

  it('is null while the producer has not published one', () => {
    assert.equal(transcriptKeyFor(panes, `${scope}${SCOPE_SEP}a2`), null);
  });
});

describe('inputKey', () => {
  it('matches the hub key shape scope\\x1fpane\\x1ffield', () => {
    assert.equal(inputKey('s', 'p', 'compose'), `s${SCOPE_SEP}p${SCOPE_SEP}compose`);
  });
});
