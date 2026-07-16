import assert from "node:assert/strict";
import fs from "node:fs";

const workflow = fs.readFileSync(process.argv[2], "utf8");
const marker = "          script: |\n";
const markerOffset = workflow.indexOf(marker);
assert.notEqual(markerOffset, -1, "watcher script is missing");
const script = workflow
  .slice(markerOffset + marker.length)
  .split("\n")
  .map((line) => (line.startsWith("            ") ? line.slice(12) : line))
  .join("\n");
const AsyncFunction = Object.getPrototypeOf(async function () {}).constructor;
const execute = new AsyncFunction(
  "github",
  "context",
  "core",
  "fetch",
  "process",
  "Buffer",
  script,
);

const now = Date.now();
const lock = {
  root: "root",
  nodes: {
    root: { inputs: {} },
  },
};

async function runScenario(overrides = {}) {
  const scenario = {
    result: "success",
    pulls: [],
    checkRuns: [],
    statuses: [],
    issues: [],
    branchSha: "",
    aheadBy: 0,
    locks: { main: lock },
    thresholdHours: 6,
    ...overrides,
  };
  const calls = {
    comments: [],
    createdIssues: [],
    failed: [],
    notices: [],
    slack: [],
    updates: [],
  };

  const endpoints = {
    pullsList: async () => {},
    checksList: async () => {},
    statusesList: async () => {},
    issuesList: async () => {},
  };
  const github = {
    paginate: async (endpoint, _params, mapFn) => {
      assert.equal(mapFn, undefined, "watcher supplied a custom Octokit pagination mapper");
      if (endpoint === endpoints.pullsList) return scenario.pulls;
      if (endpoint === endpoints.checksList) return scenario.checkRuns;
      if (endpoint === endpoints.statusesList) return scenario.statuses;
      if (endpoint === endpoints.issuesList) return scenario.issues;
      throw new Error("unexpected paginated endpoint");
    },
    rest: {
      pulls: { list: endpoints.pullsList },
      checks: { listForRef: endpoints.checksList },
      repos: {
        listCommitStatusesForRef: endpoints.statusesList,
        compareCommitsWithBasehead: async () => ({ data: { ahead_by: scenario.aheadBy } }),
        getContent: async ({ ref }) => ({
          data: {
            content: Buffer.from(JSON.stringify(scenario.locks[ref] || lock)).toString("base64"),
          },
        }),
      },
      git: {
        getRef: async () => {
          if (scenario.branchSha) return { data: { object: { sha: scenario.branchSha } } };
          throw Object.assign(new Error("not found"), { status: 404 });
        },
      },
      issues: {
        updateLabel: async () => {},
        createLabel: async () => {},
        listForRepo: endpoints.issuesList,
        create: async (request) => {
          calls.createdIssues.push(request);
          return {
            data: {
              number: 41,
              state: "open",
              body: request.body,
              html_url: "https://github.com/indexable-inc/index/issues/41",
            },
          };
        },
        update: async (request) => {
          calls.updates.push(request);
          return {
            data: {
              number: request.issue_number,
              state: request.state || "open",
              body: request.body || "",
              html_url: `https://github.com/indexable-inc/index/issues/${request.issue_number}`,
            },
          };
        },
        createComment: async (request) => {
          calls.comments.push(request);
        },
      },
    },
  };
  const core = {
    notice: (message) => calls.notices.push(message),
    warning: () => {},
    setFailed: (message) => calls.failed.push(message),
  };
  const fetch = async (_url, request) => {
    calls.slack.push(JSON.parse(request.body));
    return { ok: true, json: async () => ({ ok: true }) };
  };
  const processScope = {
    env: {
      ESCALATE_AFTER_HOURS: String(scenario.thresholdHours),
      SLACK_BOT_TOKEN: "test-slack-token",
      SLACK_CHANNEL: "C0123456789",
      UPDATE_RESULT: scenario.result,
    },
  };
  await execute(
    github,
    { repo: { owner: "indexable-inc", repo: "index" }, runId: 99 },
    core,
    fetch,
    processScope,
    Buffer,
  );
  return calls;
}

const youngPr = {
  number: 7,
  html_url: "https://github.com/indexable-inc/index/pull/7",
  created_at: new Date(now - 60_000).toISOString(),
  head: { sha: "bump-sha" },
};

{
  const calls = await runScenario({
    pulls: [youngPr],
    statuses: [
      { context: "ci", state: "success" },
      { context: "ci", state: "failure" },
    ],
    locks: { main: lock, "bump-sha": lock },
  });
  assert.deepEqual(calls.failed, [], "stale status history made a healthy PR red");
  assert.match(calls.notices[0], /healthy/);
}

{
  const oldPr = {
    ...youngPr,
    created_at: new Date(now - 7 * 3_600_000).toISOString(),
  };
  const calls = await runScenario({
    pulls: [oldPr],
    statuses: [{ context: "ci", state: "success" }],
    locks: { main: lock, "bump-sha": lock },
  });
  assert.equal(calls.createdIssues.length, 1);
  assert.match(calls.createdIssues[0].body, /checks green but the PR is still unmerged/);
  assert.equal(calls.failed.length, 1);
}

{
  const calls = await runScenario({
    result: "failure",
    thresholdHours: 0,
    issues: [
      {
        number: 9,
        state: "closed",
        body: "<!-- first-detected: 2020-01-01T00:00:00Z -->\n<!-- slack-escalated: 2020-01-01T01:00:00Z -->",
        html_url: "https://github.com/indexable-inc/index/issues/9",
      },
    ],
  });
  assert.equal(calls.slack.length, 1, "closed episode suppressed a new Slack escalation");
  assert.ok(calls.updates.every((call) => !call.body?.includes("2020-01-01")));
  assert.ok(calls.updates.at(-1).body.includes("<!-- slack-escalated:"));
}

{
  const calls = await runScenario({ result: "failure" });
  assert.equal(calls.createdIssues.length, 1);
  assert.match(calls.createdIssues[0].body, /Bump job did not succeed/);
  assert.equal(calls.failed.length, 1);
}
