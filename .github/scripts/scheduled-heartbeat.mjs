// Does every scheduled workflow in this repository still fire?
//
// A cron that stops firing records nothing. GitHub writes no run, no
// annotation and no event, so the only evidence is a hole in a list nobody
// reads. This repository has already paid for that: `cve-scan.yml`'s schedule
// did not fire at all between 2026-07-13 and 07-24 -- twelve days, zero
// scheduled runs, while the same file ran 115 times on push and pull-request
// triggers with an unchanged cron and `state=active` (ENG-11174, cause still
// unexplained). Fork sync then failed silently for four days.
//
// Split into `evaluate` (decides) and `heartbeat` (talks to the API and files
// the issue) so the deciding half is testable without a network, which is what
// `scheduled-heartbeat.test.mjs` drives.

export const WINDOW_MINUTES = 21 * 24 * 60;

// Two fires, OR this floor, whichever is more forgiving.
//
// "Has its cron come round twice" alone is too tight, and that is measured
// rather than assumed. Over the last 40 scheduled runs of each workflow in
// this repository, observed gap versus nominal period on 2026-07-29:
//
//   */15    median    50m  max    85m   (nominal    15m)
//   hourly  median    59m  max   123m   (nominal    60m)
//   */3h    median   178m  max   216m   (nominal   180m)
//   daily   median  1440m  max  1463m   (nominal  1440m)
//   weekly  median 10080m  max 10090m   (nominal 10080m)
//
// GitHub honours crons of an hour or more and drops roughly two fires in three
// of a `*/15`. Two periods alone would report `cache-push-watchdog` constantly,
// and an hourly workflow at its observed 123-minute worst case would trip a
// 120-minute deadline.
//
// Six hours would clear all of that. This is twenty-four, because of where the
// finding now goes: it fails a pull request belonging to someone who did not
// cause it. The cost of a false block is a person's afternoon and, worse, the
// credibility of the check; the cost of eighteen more hours of latency on a
// dead cron is close to nothing next to the twelve-day outage this exists to
// catch. When the two are not symmetric, buy the quiet.
export const STALE_FLOOR_MS = 24 * 60 * 60 * 1000;

// An accepted, dated, owned exemption, declared next to the schedule it
// excuses:
//
//   on:
//     # heartbeat-accepted-stale: owner=@someone until=2026-08-15 reason=ENG-11184
//     schedule:
//       - cron: "*/15 * * * *"
//
// This exists because the alternative to a legitimate escape hatch is not
// compliance, it is someone deleting the check. Three alarms in this repository
// were correct, visible and ignored; the useful thing a gate can do is convert
// "ignore it" into "record a decision", which is what today's CVE removal was.
// So accepting a dead schedule is one line, in the file it is about, carrying a
// name and an expiry -- and the expiry is enforced, because an exemption that
// never lapses is a mute button.
const EXEMPTION = /^\s*#\s*heartbeat-accepted-stale:\s*(.+?)\s*$/;

export const exemptionIn = (source) => {
  for (const line of source.split('\n')) {
    const m = EXEMPTION.exec(line);
    if (!m) continue;
    const fields = Object.fromEntries(
      m[1].split(/\s+/).flatMap((pair) => {
        const at = pair.indexOf('=');
        return at === -1 ? [] : [[pair.slice(0, at), pair.slice(at + 1)]];
      }));
    return {
      owner: fields.owner || null,
      until: fields.until || null,
      reason: fields.reason || null,
      raw: m[1],
    };
  }
  return null;
};

// One cron field to the set of values it admits. Handles `*`, `a`, `a-b`, and
// any of those with a `/step`, comma-joined. Returns null on anything it does
// not understand, so an unparseable cron is reported rather than assumed fine.
const fieldValues = (spec, min, max) => {
  const values = new Set();
  for (const part of spec.split(',')) {
    const [range, stepText] = part.split('/');
    const step = stepText ? Number(stepText) : 1;
    if (!Number.isInteger(step) || step < 1) return null;
    let lo;
    let hi;
    if (range === '*') {
      lo = min;
      hi = max;
    } else if (range.includes('-')) {
      const [a, b] = range.split('-');
      lo = Number(a);
      hi = Number(b);
    } else {
      lo = Number(range);
      // `5/2` means "from 5 to the end, every 2"; a bare `5` is just 5.
      hi = stepText ? max : Number(range);
    }
    if (![lo, hi].every(Number.isInteger) || lo < min || hi > max || lo > hi) return null;
    for (let v = lo; v <= hi; v += step) values.add(v);
  }
  return values;
};

// Parsed once per cron rather than once per candidate minute: the backscan can
// walk 30,240 minutes for a weekly cron.
export const compile = (cron) => {
  const parts = cron.trim().split(/\s+/);
  if (parts.length !== 5) return null;
  const [minute, hour, dom, month, dowRaw] = parts;
  // GitHub accepts 7 for Sunday; normalise to match Date#getUTCDay.
  const dow = dowRaw.replace(/7/g, '0');
  const fields = [
    fieldValues(minute, 0, 59),
    fieldValues(hour, 0, 23),
    fieldValues(dom, 1, 31),
    fieldValues(month, 1, 12),
    fieldValues(dow, 0, 6),
  ];
  if (fields.some((f) => f === null)) return null;
  return {
    minute: fields[0],
    hour: fields[1],
    dom: fields[2],
    month: fields[3],
    dow: fields[4],
    domRestricted: dom !== '*',
    dowRestricted: dow !== '*',
  };
};

const fires = (c, d) => {
  if (!c.minute.has(d.getUTCMinutes())) return false;
  if (!c.hour.has(d.getUTCHours())) return false;
  if (!c.month.has(d.getUTCMonth() + 1)) return false;
  const domHit = c.dom.has(d.getUTCDate());
  const dowHit = c.dow.has(d.getUTCDay());
  // POSIX: when BOTH day-of-month and day-of-week are restricted the match is
  // a union, not an intersection.
  if (c.domRestricted && c.dowRestricted) return domHit || dowHit;
  return domHit && dowHit;
};

// The `want` most recent times these crons should have fired before `now`,
// newest first, searching back at most WINDOW_MINUTES.
export const recentFires = (compiled, want, now) => {
  const found = [];
  const cursor = new Date(Date.UTC(
    now.getUTCFullYear(), now.getUTCMonth(), now.getUTCDate(),
    now.getUTCHours(), now.getUTCMinutes()));
  for (let i = 0; i < WINDOW_MINUTES && found.length < want; i += 1) {
    cursor.setUTCMinutes(cursor.getUTCMinutes() - 1);
    if (compiled.some((c) => fires(c, cursor))) found.push(new Date(cursor));
  }
  return found;
};

// The anchor is load-bearing, not decoration. `update-flake-lock.yml` carries
// a deliberately commented-out `#   - cron: "17 * * * *"` under a paragraph
// explaining why it is dispatch-only, and an unanchored search reads that as a
// live hourly schedule nine days overdue -- which is exactly the false alarm
// that got raised by hand while writing this. Requiring only whitespace and an
// optional list dash before `cron:` rejects every commented form, so no
// separate comment filter is needed; one written here first turned out to be
// unreachable.
export const cronsIn = (source) =>
  source.split('\n').flatMap((line) => {
    const m = /^\s*-?\s*cron:\s*['"]([^'"]+)['"]/.exec(line);
    return m ? [m[1]] : [];
  });

export async function evaluate({workflows, readFile, latestScheduledRun, now}) {
  const problems = [];
  const skipped = [];
  const expired = [];
  let checked = 0;

  for (const workflow of workflows) {
    let source;
    try {
      source = await readFile(workflow.path);
    } catch {
      // Known to the API but absent from the default branch: deleted, or
      // defined on another branch. Neither is this check's business.
      continue;
    }
    const crons = cronsIn(source);
    if (crons.length === 0) continue;

    const compiled = crons.map(compile);
    if (compiled.some((c) => c === null)) {
      problems.push(`\`${workflow.path}\`: cron expression is unparseable (\`${crons.join('`, `')}\`), so its schedule cannot be verified`);
      continue;
    }

    // A schedule GitHub has switched off fires nothing and says so nowhere
    // else. `disabled_inactivity` in particular is silent.
    if (workflow.state !== 'active') {
      problems.push(`\`${workflow.path}\`: workflow state is \`${workflow.state}\`, so its schedule does not run`);
      continue;
    }

    const fireTimes = recentFires(compiled, 2, now);
    if (fireTimes.length < 2) {
      skipped.push(`\`${workflow.path}\`: fires less often than twice per ${WINDOW_MINUTES / (24 * 60)} days, so staleness is not decidable from this window`);
      continue;
    }
    const deadline = new Date(
      Math.min(fireTimes[1].getTime(), now.getTime() - STALE_FLOOR_MS));

    // A live exemption short-circuits before the run query, so an accepted dead
    // cron costs nothing. A lapsed one does NOT short-circuit and does not
    // report on its own either: it only matters if the thing it excused is
    // still broken, so it changes what the staleness finding says rather than
    // adding a second one beside it. An exemption that lapsed after the
    // workflow recovered is untidy, not a reason to fail somebody's pull
    // request.
    let lapsed = null;
    const excuse = exemptionIn(source);
    if (excuse) {
      const until = excuse.until ? Date.parse(`${excuse.until}T23:59:59Z`) : NaN;
      if (!excuse.owner || Number.isNaN(until)) {
        problems.push(`\`${workflow.path}\`: heartbeat-accepted-stale needs \`owner=\` and \`until=YYYY-MM-DD\`, got \`${excuse.raw}\``);
        continue;
      }
      if (until >= now.getTime()) {
        skipped.push(`\`${workflow.path}\`: staleness accepted by ${excuse.owner} until ${excuse.until}${excuse.reason ? ` (${excuse.reason})` : ''}`);
        continue;
      }
      lapsed = excuse;
    }

    // Younger than the deadline: it has not had two chances yet. Not a fault,
    // but say so rather than counting it as healthy.
    if (Date.parse(workflow.created_at) > deadline.getTime()) {
      skipped.push(`\`${workflow.path}\`: created ${workflow.created_at}, has not had two scheduled fires yet`);
      continue;
    }

    const latest = await latestScheduledRun(workflow);
    checked += 1;
    const iso = deadline.toISOString();
    const stale = !latest || Date.parse(latest.created_at) < deadline.getTime();
    if (!stale) continue;

    const detail = latest
      ? `last scheduled run ${latest.created_at} ([${latest.id}](${latest.html_url})), expected one at or after ${iso}`
      : `no scheduled run has ever been recorded, and its cron has fired at least twice (most recently ${iso})`;
    if (lapsed) {
      expired.push(`\`${workflow.path}\`: still not firing, and the acceptance by ${lapsed.owner} ran out on ${lapsed.until}${lapsed.reason ? ` (${lapsed.reason})` : ''} -- ${detail}`);
    } else {
      problems.push(`\`${workflow.path}\`: ${detail}`);
    }
  }

  return {problems, skipped, expired, checked};
}

// Where the finding goes, which is the hard half.
//
// Detection is easy; being read is not. This repository has three proofs that
// a correct, visible alarm changes nothing:
//
//   - fork-sync failed for four days, red on every run, logs legible. It was
//     found by an agent watching #4032's post-merge CI for another reason.
//   - The CVE watchdog failed 100+ consecutive times, correctly, filing and
//     updating #3834 hourly for eight days. Nothing happened until someone was
//     told to go and look.
//   - #4045 and #4276 are the same root cause filed six days apart. The second
//     spells out that an org owner must grant one permission, and it is still
//     open.
//
// The Actions tab and the issue tracker are both channels this repository has
// learned to ignore, and the third proof rules out "write a better ticket" as
// the fix. So this files nothing and sets no scheduled red. It runs on pull
// requests and fails the check there, because the one thing observed to produce
// a decision inside a day is cost landing on somebody who is already trying to
// get something done -- the CVE ratchet was measured and turned off within a day
// of costing 2,465s per PR, while the watchdog's ticket sat for eight.
//
// Honest about the decay mode: that decision may well be "disable this". A
// legitimate, owned, dated exemption is therefore built in, so the cheap move
// is recording the acceptance rather than deleting the gate. Converting
// "ignore" into "a decision with a name on it" is the most this mechanism can
// honestly claim, and it is what today's CVE removal was.
//
// Running on pull_request also removes the turtle problem: a heartbeat with no
// cron of its own has no schedule that can silently die. If nobody is opening
// pull requests, nobody is working, and a stale cron is not the urgent thing.
export default async function heartbeat({github, context, core}) {
  const fs = await import('node:fs/promises');
  const {owner, repo} = context.repo;

  const workflows = await github.paginate(
    github.rest.actions.listRepoWorkflows, {owner, repo, per_page: 100});

  const {problems, skipped, expired, checked} = await evaluate({
    workflows,
    now: new Date(),
    readFile: (p) => fs.readFile(p, 'utf8'),
    latestScheduledRun: async (workflow) => {
      const runs = await github.rest.actions.listWorkflowRuns({
        owner, repo, workflow_id: workflow.id, event: 'schedule', per_page: 1,
      });
      return runs.data.workflow_runs[0] || null;
    },
  });

  for (const note of skipped) core.notice(`heartbeat skipped ${note}`);

  const findings = [...problems, ...expired];
  if (findings.length === 0) {
    core.notice(`${checked} scheduled workflows checked, all firing`);
    return;
  }

  // Written to the step summary as well as the failure, so the detail is on the
  // PR's checks page rather than only inside a log nobody opens.
  await core.summary
    .addHeading('A scheduled workflow has stopped firing')
    .addRaw([
      'This is **not about your change.** A cron in this repository has not fired',
      'for at least a day, and this check runs on pull requests because that is the',
      'only place a finding in this repository has ever been acted on.',
      '',
      ...findings.map((f) => `- ${f}`),
      '',
      'Clear it by doing one of these:',
      '',
      '1. Fix the workflow, or re-dispatch it and confirm the schedule resumes.',
      '2. Delete its `schedule:` trigger if it should not be running.',
      '3. Accept it, in the workflow file, next to the schedule:',
      '',
      '   ```yaml',
      '   on:',
      '     # heartbeat-accepted-stale: owner=@you until=2026-08-15 reason=ENG-1234',
      '     schedule:',
      '       - cron: "..."',
      '   ```',
      '',
      'The expiry is enforced. An exemption that has lapsed is reported the same',
      'way a dead cron is, so accepting is a decision with a name and a date on it',
      'rather than a mute button.',
    ].join('\n'))
    .write();

  core.setFailed(
    `${findings.length} scheduled workflow(s) have stopped firing (not caused by this PR): ` +
    findings.map((f) => f.split(':')[0]).join(', '));
}
