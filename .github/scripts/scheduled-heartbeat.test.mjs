// The deciding half of the heartbeat, driven without a network.
//
// Every boundary here is anchored to a computed fire time rather than a
// wall-clock offset. Writing them as offsets got three cases wrong while this
// was being built: "70 minutes ago" for a `47 * * * *` cron lands BEFORE the
// previous fire, so it is two misses and not one, and "30 hours ago" for a
// `13 6 * * *` cron is likewise two and not one. An off-by-one-period test is
// worse than no test, because it argues for changing correct code.
import {test} from 'node:test';
import assert from 'node:assert/strict';
import {compile, cronsIn, evaluate, exemptionIn, recentFires} from './scheduled-heartbeat.mjs';

const MIN = 60 * 1000;
const NOW = new Date('2026-07-29T11:00:00Z');

const HOURLY = '47 * * * *';
const QUARTER = '*/15 * * * *';
const DAILY = '13 6 * * *';
const WEEKLY = '17 4 * * 1';

const fireBack = (cron, n) => recentFires([compile(cron)], n, NOW)[n - 1];

const world = ({cron = HOURLY, state = 'active', created = '2026-01-01T00:00:00Z', run, accept = ''}) => ({
  workflows: [{path: '.github/workflows/subject.yml', id: 1, state, created_at: created}],
  readFile: async () => `on:\n${accept}  schedule:\n    - cron: "${cron}"\n`,
  latestScheduledRun: async () => run,
  now: NOW,
});

const reported = async (opts) => (await evaluate(world(opts))).problems.length > 0;

test('cron fire times match the nominal period', () => {
  const q = recentFires([compile(QUARTER)], 2, NOW);
  assert.equal((q[0] - q[1]) / MIN, 15);
  const h = recentFires([compile(HOURLY)], 2, NOW);
  assert.equal((h[0] - h[1]) / MIN, 60);
  const d = recentFires([compile(DAILY)], 2, NOW);
  assert.equal((d[0] - d[1]) / MIN, 24 * 60);
  const w = recentFires([compile(WEEKLY)], 2, NOW);
  assert.equal((w[0] - w[1]) / MIN, 7 * 24 * 60);
  // Monday, as `* * 1` asks for.
  assert.equal(w[0].getUTCDay(), 1);
});

test('a commented-out cron is not a schedule', () => {
  assert.deepEqual(cronsIn('on:\n  # schedule:\n  #   - cron: "17 * * * *"\n'), []);
  assert.deepEqual(cronsIn('on:\n  schedule:\n    - cron: "17 * * * *"\n'), ['17 * * * *']);
});

test('an unparseable cron is reported, never assumed healthy', async () => {
  assert.equal(compile('not a cron'), null);
  assert.equal(compile('61 * * * *'), null);
  assert.ok(await reported({cron: 'not a cron', run: {id: 1, created_at: NOW.toISOString()}}));
});

test('a fresh run is clean', async () => {
  assert.equal(await reported({run: {id: 1, created_at: NOW.toISOString()}}), false);
});

test('hourly: one missed fire is the slack, not a fault', async () => {
  // Ran exactly on the second-most-recent fire: only the latest was missed.
  const run = {id: 1, created_at: fireBack(HOURLY, 2).toISOString()};
  assert.equal(await reported({run}), false);
});

test('hourly: twenty hours of silence is under the stale floor', async () => {
  // Under the floor on purpose. This fails a pull request belonging to someone
  // who did not cause it, so latency is the cheap side of the trade.
  const run = {id: 1, created_at: new Date(NOW - 20 * 60 * MIN).toISOString()};
  assert.equal(await reported({run}), false);
});

test('hourly: twenty-six hours of silence is over the stale floor', async () => {
  const run = {id: 1, created_at: new Date(NOW - 26 * 60 * MIN).toISOString()};
  assert.equal(await reported({run}), true);
});

test('*/15 at its worst observed real gap stays clean', async () => {
  // 85 minutes is the maximum gap actually measured for cache-push-watchdog.
  const run = {id: 1, created_at: new Date(NOW - 85 * MIN).toISOString()};
  assert.equal(await reported({cron: QUARTER, run}), false);
});

test('daily: one missed fire is clean, two is reported', async () => {
  assert.equal(
    await reported({cron: DAILY, run: {id: 1, created_at: fireBack(DAILY, 2).toISOString()}}),
    false);
  assert.equal(
    await reported({cron: DAILY, run: {id: 1, created_at: new Date(fireBack(DAILY, 2) - MIN).toISOString()}}),
    true);
});

test('the twelve-day shape that motivated this check is reported', async () => {
  const run = {id: 1, created_at: new Date(NOW - 12 * 24 * 60 * MIN).toISOString()};
  assert.equal(await reported({run}), true);
});

test('never having run at all is reported', async () => {
  assert.equal(await reported({run: null}), true);
});

test('a workflow GitHub has disabled is reported', async () => {
  assert.equal(
    await reported({state: 'disabled_inactivity', run: {id: 1, created_at: NOW.toISOString()}}),
    true);
});

test('a workflow younger than its own deadline is skipped, not reported', async () => {
  const {problems, skipped} = await evaluate(world({
    created: new Date(NOW - 5 * MIN).toISOString(),
    run: null,
  }));
  assert.deepEqual(problems, []);
  assert.equal(skipped.length, 1);
  assert.match(skipped[0], /has not had two scheduled fires yet/);
});

test('a cron too rare for the window is skipped loudly, not passed', async () => {
  const {problems, skipped} = await evaluate(world({cron: '0 0 1 1 *', run: null}));
  assert.deepEqual(problems, []);
  assert.match(skipped[0], /not decidable from this window/);
});

test('a workflow with no file in the checkout is ignored', async () => {
  const {problems, skipped, checked} = await evaluate({
    workflows: [{path: 'dynamic/dependabot/update-graph', id: 1, state: 'active', created_at: '2026-01-01T00:00:00Z'}],
    readFile: async () => { throw new Error('ENOENT'); },
    latestScheduledRun: async () => null,
    now: NOW,
  });
  assert.deepEqual(problems, []);
  assert.deepEqual(skipped, []);
  assert.equal(checked, 0);
});

test('a workflow with no schedule at all is ignored', async () => {
  const {problems, checked} = await evaluate({
    workflows: [{path: '.github/workflows/check.yml', id: 1, state: 'active', created_at: '2026-01-01T00:00:00Z'}],
    readFile: async () => 'on:\n  pull_request:\n',
    latestScheduledRun: async () => null,
    now: NOW,
  });
  assert.deepEqual(problems, []);
  assert.equal(checked, 0);
});

const ACCEPT = (fields) => `  # heartbeat-accepted-stale: ${fields}\n`;
const dead = {id: 1, created_at: new Date(NOW - 12 * 24 * 60 * MIN).toISOString()};

test('an exemption is parsed into owner, until and reason', () => {
  const e = exemptionIn('on:\n  # heartbeat-accepted-stale: owner=@a until=2026-08-15 reason=ENG-1\n');
  assert.equal(e.owner, '@a');
  assert.equal(e.until, '2026-08-15');
  assert.equal(e.reason, 'ENG-1');
  assert.equal(exemptionIn('on:\n  schedule:\n'), null);
});

test('a live exemption silences a dead cron', async () => {
  const {problems, skipped} = await evaluate(world({
    run: dead, accept: ACCEPT('owner=@a until=2026-08-15 reason=ENG-1'),
  }));
  assert.deepEqual(problems, []);
  assert.match(skipped[0], /accepted by @a until 2026-08-15/);
});

test('a lapsed exemption over a still-dead cron is one finding, not two', async () => {
  const {problems, expired} = await evaluate(world({
    run: dead, accept: ACCEPT('owner=@a until=2026-07-01 reason=ENG-1'),
  }));
  assert.deepEqual(problems, []);
  assert.equal(expired.length, 1);
  assert.match(expired[0], /still not firing, and the acceptance by @a ran out on 2026-07-01/);
});

test('a lapsed exemption over a recovered cron is not a finding at all', async () => {
  // Untidy, not a reason to fail somebody's unrelated pull request.
  const {problems, expired} = await evaluate(world({
    run: {id: 1, created_at: NOW.toISOString()},
    accept: ACCEPT('owner=@a until=2026-07-01 reason=ENG-1'),
  }));
  assert.deepEqual(problems, []);
  assert.deepEqual(expired, []);
});

test('an exemption without an owner or an expiry is rejected', async () => {
  for (const fields of ['reason=ENG-1', 'owner=@a', 'owner=@a until=soon']) {
    const {problems} = await evaluate(world({run: dead, accept: ACCEPT(fields)}));
    assert.equal(problems.length, 1, fields);
    assert.match(problems[0], /needs `owner=` and `until=YYYY-MM-DD`/);
  }
});

test('an exemption does not silence a healthy workflow into a skip', async () => {
  // Exempt but alive: still skipped rather than checked, which is fine, but it
  // must not be counted as a problem.
  const {problems} = await evaluate(world({
    run: {id: 1, created_at: NOW.toISOString()},
    accept: ACCEPT('owner=@a until=2026-08-15'),
  }));
  assert.deepEqual(problems, []);
});
