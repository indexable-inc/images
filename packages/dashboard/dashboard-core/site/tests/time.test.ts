import assert from 'node:assert/strict';
import { describe, it } from 'node:test';

import { humanTime, humanDuration, runTooltip, humanAge } from '../src/lib/time.ts';

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
