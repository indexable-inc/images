import assert from 'node:assert/strict';
import { describe, it } from 'node:test';

import { humanTime, humanDuration, runTooltip, humanAge, humanDate, recordingLabel } from '../src/lib/time.ts';

const MIN = 60_000;
const HOUR = 60 * MIN;
const DAY = 24 * HOUR;

describe('humanTime', () => {
  const ref = new Date('2026-07-02T14:32:00').getTime();

  it('shows a relative age under a minute (never 0s)', () => {
    assert.equal(humanTime(ref - 12_000, ref), '12s ago');
    assert.equal(humanTime(ref, ref), '1s ago');
  });

  it('shows minutes under an hour', () => {
    assert.equal(humanTime(ref - 3 * MIN, ref), '3m ago');
    assert.equal(humanTime(ref - 59 * MIN, ref), '59m ago');
  });

  it('shows a same-day clock time once over an hour old', () => {
    const out = humanTime(ref - 3 * HOUR, ref);
    // A HH:MM 24-hour stamp, not a relative age.
    assert.match(out, /^\d{1,2}:\d{2}$/);
    assert.doesNotMatch(out, /ago/);
  });

  it('shows a short date for anything older than today', () => {
    const out = humanTime(ref - 3 * DAY, ref);
    // A "Mon D" style date (month abbreviation + day), not a clock time.
    assert.match(out, /[A-Za-z]/);
    assert.doesNotMatch(out, /ago/);
    assert.doesNotMatch(out, /^\d{1,2}:\d{2}$/);
  });

  it('is empty for a missing start', () => {
    assert.equal(humanTime(undefined, ref), '');
  });
});

describe('humanDuration', () => {
  it('formats sub-second, seconds, and minutes', () => {
    assert.equal(humanDuration(420), '420ms');
    assert.equal(humanDuration(1300), '1.3s');
    assert.equal(humanDuration(9800), '9.8s');
    assert.equal(humanDuration(44_000), '44s');
    assert.equal(humanDuration(64_000), '1m4s');
  });

  it('is empty for absent or negative input', () => {
    assert.equal(humanDuration(undefined), '');
    assert.equal(humanDuration(-1), '');
  });
});

describe('runTooltip', () => {
  const ref = 1_000_000;

  it('reports elapsed time while running', () => {
    assert.equal(runTooltip(true, undefined, ref - 12_000, ref), 'running · 12s elapsed');
  });

  it('reports the finished duration otherwise', () => {
    assert.equal(runTooltip(false, 2300, ref - 5000, ref), 'took 2.3s');
  });

  it('is empty for a finished run with no duration', () => {
    assert.equal(runTooltip(false, undefined, ref, ref), '');
  });
});

describe('humanDate', () => {
  // Anchor the reference at a fixed local wall-clock so the today/yesterday
  // windows are calendar-day based, not a rolling 24h.
  const ref = new Date('2026-07-02T14:32:00').getTime();

  it('labels the reference day "today" and the prior day "yesterday"', () => {
    assert.equal(humanDate(new Date('2026-07-02T09:00:00').getTime(), ref), 'today');
    assert.equal(humanDate(new Date('2026-07-01T23:00:00').getTime(), ref), 'yesterday');
  });

  it('shows a short date within the year and adds the year otherwise', () => {
    assert.equal(humanDate(new Date('2026-06-20T10:00:00').getTime(), ref), 'Jun 20');
    assert.match(humanDate(new Date('2025-06-20T10:00:00').getTime(), ref), /2025/);
  });

  it('is empty for a missing timestamp', () => {
    assert.equal(humanDate(0, ref), '');
  });

  it('counts calendar days across a DST boundary (whole-day, not 24h math)', () => {
    // US spring-forward 2026 is Mar 8; the day before is only 23h long, which a
    // naive (midnight - midnight)/86_400_000 floor would count as 0 days and
    // mislabel as "today". Anchor "now" on Mar 9 and the start on Mar 8.
    const now = new Date('2026-03-09T10:00:00').getTime();
    assert.equal(humanDate(new Date('2026-03-09T01:00:00').getTime(), now), 'today');
    assert.equal(humanDate(new Date('2026-03-08T23:30:00').getTime(), now), 'yesterday');
  });
});

describe('recordingLabel', () => {
  const ref = new Date('2026-07-02T14:32:00').getTime();
  const start = new Date('2026-07-02T11:53:00').getTime();

  it('joins the start (date + clock) with the run duration', () => {
    const out = recordingLabel(start, start + 47 * MIN + 12_000, ref);
    assert.match(out, /^today \d{1,2}:\d{2} · 47m12s$/);
  });

  it('omits a sub-second span (a single-snapshot recording)', () => {
    const out = recordingLabel(start, start + 200, ref);
    assert.match(out, /^today \d{1,2}:\d{2}$/);
    assert.doesNotMatch(out, /·/);
  });
});

describe('humanAge', () => {
  it('rounds to now / seconds / minutes / hours / days', () => {
    const ref = 10 * DAY;
    assert.equal(humanAge(ref, ref), 'now');
    assert.equal(humanAge(ref - 5000, ref), '5s ago');
    assert.equal(humanAge(ref - 5 * MIN, ref), '5m ago');
    assert.equal(humanAge(ref - 5 * HOUR, ref), '5h ago');
    assert.equal(humanAge(ref - 5 * DAY, ref), '5d ago');
  });
});
