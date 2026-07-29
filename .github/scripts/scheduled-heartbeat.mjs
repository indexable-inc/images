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
// of a `*/15`. Two periods would therefore report `cache-push-watchdog`
// constantly, and an hourly workflow at its observed 123-minute worst case
// would trip a 120-minute deadline. A check that cries wolf whenever GitHub is
// busy is a check people mute, and a muted check is what hid the outage this
// exists to catch. Six hours clears every observed gap with room and still
// catches a multi-day stoppage within a working morning; for daily and weekly
// crons the two-fire rule dominates and the floor never binds.
export const JITTER_FLOOR_MS = 6 * 60 * 60 * 1000;

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
      Math.min(fireTimes[1].getTime(), now.getTime() - JITTER_FLOOR_MS));

    // Younger than the deadline: it has not had two chances yet. Not a fault,
    // but say so rather than counting it as healthy.
    if (Date.parse(workflow.created_at) > deadline.getTime()) {
      skipped.push(`\`${workflow.path}\`: created ${workflow.created_at}, has not had two scheduled fires yet`);
      continue;
    }

    const latest = await latestScheduledRun(workflow);
    checked += 1;
    const iso = deadline.toISOString();
    if (!latest) {
      problems.push(`\`${workflow.path}\`: no scheduled run has ever been recorded, and its cron has fired at least twice (most recently ${iso})`);
    } else if (Date.parse(latest.created_at) < deadline.getTime()) {
      problems.push(`\`${workflow.path}\`: cron has come round twice with nothing since. Last scheduled run ${latest.created_at} ([${latest.id}](${latest.html_url})), expected one at or after ${iso}`);
    }
  }

  return {problems, skipped, checked};
}

export const MARKER = '<!-- scheduled-workflow-heartbeat -->';

export default async function heartbeat({github, context, core}) {
  const fs = await import('node:fs/promises');
  const {owner, repo} = context.repo;

  const workflows = await github.paginate(
    github.rest.actions.listRepoWorkflows, {owner, repo, per_page: 100});

  const {problems, skipped, checked} = await evaluate({
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

  // Filtered by label rather than sweeping every issue: this runs hourly AND
  // on every push to main, and an unfiltered `state: 'all'` paginate is ~60
  // calls a time to read one bit.
  const issues = await github.rest.issues.listForRepo({
    owner, repo, state: 'all', labels: 'github_actions', per_page: 100,
  });
  const issue = issues.data.find((candidate) =>
    !candidate.pull_request && (candidate.body || '').includes(MARKER));

  if (problems.length === 0) {
    if (issue && issue.state === 'open') {
      await github.rest.issues.createComment({
        owner, repo, issue_number: issue.number,
        body: `Every scheduled workflow has fired within its own period again (${checked} checked). Closing automatically.\n\n(sent by the scheduled workflow heartbeat)`,
      });
      await github.rest.issues.update({
        owner, repo, issue_number: issue.number, state: 'closed',
      });
    }
    core.notice(`${checked} scheduled workflows checked, all firing`);
    return;
  }

  const runUrl = `${context.serverUrl}/${owner}/${repo}/actions/runs/${context.runId}`;
  const body = [
    MARKER,
    '## A scheduled workflow has stopped firing',
    '',
    'Each entry below has had its cron come round at least twice, and at least',
    'six hours pass, with no scheduled run since. GitHub records nothing when a',
    'cron does not fire, so this is the only signal there is.',
    '',
    ...problems.map((p) => `- ${p}`),
    '',
    ...(skipped.length ? ['Not decidable this run:', '', ...skipped.map((s) => `- ${s}`), ''] : []),
    `Observed by the scheduled workflow heartbeat: ${runUrl}`,
    '',
    'The cause of the twelve-day outage that motivated this check is still',
    'unexplained; see ENG-11174.',
    '',
    '(sent by the scheduled workflow heartbeat)',
  ].join('\n');

  if (issue) {
    await github.rest.issues.update({
      owner, repo, issue_number: issue.number, body,
      ...(issue.state === 'closed' ? {state: 'open'} : {}),
    });
  } else {
    await github.rest.issues.create({
      owner, repo, title: 'A scheduled workflow has stopped firing',
      body, labels: ['bug', 'github_actions'],
    });
  }
  core.setFailed(`${problems.length} scheduled workflow(s) have stopped firing`);
}
