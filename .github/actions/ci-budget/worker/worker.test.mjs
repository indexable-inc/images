import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { once } from "node:events";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { createServer } from "node:http";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { pathToFileURL } from "node:url";
import test from "node:test";

import {
  DeadlineExceeded,
  assertQueueAdmission,
  assertSetupAllowance,
  budgetExceededMessage,
  currentWorkerJob,
  loadPolicy,
  parseArguments,
  queuedRunCount,
  recordBudgetVerdicts,
  reportBudgetExceeded,
  runBudgetedScript,
  validationSeconds,
} from "./worker.mjs";

const pause = (milliseconds) =>
  new Promise((resolvePause) => setTimeout(resolvePause, milliseconds));

async function waitForFile(path) {
  for (let attempt = 0; attempt < 100; attempt += 1) {
    try {
      return await readFile(path, "utf8");
    } catch (error) {
      if (error.code !== "ENOENT") throw error;
      await pause(10);
    }
  }
  throw new Error(`timed out waiting for ${path}`);
}

async function waitForExit(child) {
  return new Promise((resolveExit, rejectExit) => {
    child.once("error", rejectExit);
    child.once("exit", (code, signal) => resolveExit({ code, signal }));
  });
}

async function assertProcessGone(pid) {
  for (let attempt = 0; attempt < 100; attempt += 1) {
    try {
      process.kill(pid, 0);
      await pause(10);
    } catch (error) {
      if (error.code === "ESRCH") return;
      throw error;
    }
  }
  assert.fail(`process ${pid} survived worker cleanup`);
}

async function processTreeFixture(directory) {
  const script = join(directory, "spawn-tree.sh");
  const descendant = join(directory, "descendant-pid");
  await writeFile(
    script,
    [
      "#!/usr/bin/env bash",
      "set -euo pipefail",
      "trap 'exit 143' TERM",
      "(trap '' TERM; sleep 60) &",
      'printf \'%s\\n\' "$!" >descendant-pid',
      "wait",
      "",
    ].join("\n"),
  );
  return { descendant, script };
}

function workerJob({
  attempt = 2,
  createdAt = "2026-07-15T10:00:00Z",
  labels = ["ix-ci-run-42-2-nix"],
  name = "nix-build",
  runnerName = "runner-42",
  startedAt = "2026-07-15T10:05:00Z",
  status = "in_progress",
} = {}) {
  return {
    created_at: createdAt,
    labels,
    name,
    run_attempt: attempt,
    runner_name: runnerName,
    started_at: startedAt,
    status,
  };
}

function response(jobs) {
  return new Response(JSON.stringify({ jobs, total_count: jobs.length }), {
    status: 200,
  });
}

test("policy exposes independent queue, setup, validation, and cleanup clocks", () => {
  const policy = loadPolicy();
  assert.equal(policy.big_change_label, "ci/big-change");
  assert.equal(policy.queue_start_seconds, 7200);
  assert.equal(policy.setup_allowance_seconds, 120);
  assert.equal(policy.routine_validation_seconds, 300);
  assert.equal(policy.extended_validation_seconds, 10_800);
  assert.equal(policy.termination_grace_seconds, 10);
});

test("GitHub action entrypoint invokes the worker under a distinct argv path", async () => {
  const entrypoint = join(import.meta.dirname, "worker-entrypoint.mjs");
  const child = spawn(process.execPath, [entrypoint], {
    env: Object.fromEntries(
      Object.entries(process.env).filter(([name]) => !name.startsWith("INPUT_")),
    ),
    stdio: ["ignore", "pipe", "pipe"],
  });
  let stderr = "";
  child.stderr.setEncoding("utf8");
  child.stderr.on("data", (chunk) => {
    stderr += chunk;
  });

  const result = await waitForExit(child);
  assert.deepEqual(result, { code: 1, signal: null });
  assert.match(stderr, /input big-change is required/);
});

test("GitHub action entrypoint runs validation and preserves script outputs", async () => {
  const directory = await mkdtemp(join(tmpdir(), "ci-budget-entrypoint-"));
  const startedAt = new Date().toISOString();
  const createdAt = new Date(Date.now() - 1000).toISOString();
  const server = createServer((request, responseStream) => {
    assert.equal(
      request.url,
      "/repos/indexable-inc/ix/actions/runs/42/attempts/1/jobs?per_page=100&page=1",
    );
    responseStream.writeHead(200, { "content-type": "application/json" });
    responseStream.end(
      JSON.stringify({
        jobs: [
          workerJob({
            attempt: 1,
            createdAt,
            runnerName: "runner-entrypoint",
            startedAt,
          }),
        ],
        total_count: 1,
      }),
    );
  });

  try {
    server.listen(0, "127.0.0.1");
    await once(server, "listening");
    const address = server.address();
    assert.notEqual(address, null);
    assert.equal(typeof address, "object");

    const script = join(directory, "validate.sh");
    const output = join(directory, "github-output");
    await writeFile(
      script,
      [
        "#!/usr/bin/env bash",
        "set -euo pipefail",
        "printf 'ran\\n' >validation-ran",
        "printf 'probe_result=success\\n' >>\"$GITHUB_OUTPUT\"",
        "",
      ].join("\n"),
    );
    await writeFile(output, "");

    const entrypoint = join(import.meta.dirname, "worker-entrypoint.mjs");
    const child = spawn(process.execPath, [entrypoint], {
      env: {
        ...Object.fromEntries(
          Object.entries(process.env).filter(([name]) => !name.startsWith("INPUT_")),
        ),
        GITHUB_API_URL: `http://127.0.0.1:${address.port}`,
        GITHUB_OUTPUT: output,
        GITHUB_WORKSPACE: directory,
        "INPUT_BIG-CHANGE": "false",
        INPUT_MODE: "run",
        INPUT_REPOSITORY: "indexable-inc/ix",
        "INPUT_RUN-ATTEMPT": "1",
        "INPUT_RUN-ID": "42",
        INPUT_SCRIPT: "validate.sh",
        INPUT_TOKEN: "test-token",
        RUNNER_NAME: "runner-entrypoint",
      },
      stdio: ["ignore", "pipe", "pipe"],
    });
    let stderr = "";
    child.stderr.setEncoding("utf8");
    child.stderr.on("data", (chunk) => {
      stderr += chunk;
    });

    const result = await waitForExit(child);
    assert.deepEqual(result, { code: 0, signal: null }, stderr);
    assert.equal(await readFile(join(directory, "validation-ran"), "utf8"), "ran\n");
    assert.equal(await readFile(output, "utf8"), "probe_result=success\n");
  } finally {
    server.close();
    await once(server, "close");
    await rm(directory, { force: true, recursive: true });
  }
});

test("worker selects the exact current attempt runner job", async () => {
  let requestedUrl;
  const job = await currentWorkerJob({
    apiUrl: "https://api.example.test",
    fetchImpl: async (url) => {
      requestedUrl = url;
      return response([
        workerJob({ name: "lint-build", runnerName: "runner-other" }),
        workerJob(),
      ]);
    },
    repository: "indexable-inc/ix",
    runAttempt: 2,
    runId: 42,
    runnerName: "runner-42",
    token: "secret",
  });

  assert.equal(job.name, "nix-build");
  assert.equal(
    requestedUrl.toString(),
    "https://api.example.test/repos/indexable-inc/ix/actions/runs/42/attempts/2/jobs?per_page=100&page=1",
  );
});

test("worker rejects an inconsistent retry identity", async () => {
  await assert.rejects(
    currentWorkerJob({
      fetchImpl: async () => response([workerJob({ attempt: 3 })]),
      repository: "indexable-inc/ix",
      runAttempt: 2,
      runId: 42,
      runnerName: "runner-42",
      token: "secret",
    }),
    /attempt 3, expected 2/,
  );
});

test("worker identity fails closed on missing and duplicate runner matches", async () => {
  for (const jobs of [
    [workerJob({ runnerName: "runner-other" })],
    [workerJob(), workerJob({ name: "lint-build" })],
  ]) {
    await assert.rejects(
      currentWorkerJob({
        fetchImpl: async () => response(jobs),
        repository: "indexable-inc/ix",
        runAttempt: 2,
        runId: 42,
        runnerName: "runner-42",
        token: "secret",
      }),
      /expected exactly one/,
    );
  }
});

test("queue admission accepts the boundary and rejects one millisecond late", () => {
  const policy = loadPolicy();
  // Created 10:00:00 plus the 7200s admission budget puts the boundary at
  // exactly 12:00:00 (#3549 widened it from 300s).
  assert.doesNotThrow(() =>
    assertQueueAdmission({
      job: workerJob({ startedAt: "2026-07-15T12:00:00Z" }),
      policy,
    }),
  );
  assert.throws(
    () =>
      assertQueueAdmission({
        job: workerJob({ startedAt: "2026-07-15T12:00:00.001Z" }),
        policy,
      }),
    DeadlineExceeded,
  );
});

test("queue admission failure reports the wait, the budget, and saturation", () => {
  const policy = loadPolicy();
  // ix#7625: a PR worker started long after creation while stacked deploy
  // runs held every dispatcher slot. The failure must read as pool
  // saturation with the actual wait, not as a bare timestamp comparison.
  // (The wait here overruns even the 7200s budget #3549 widened to.)
  assert.throws(
    () =>
      assertQueueAdmission({
        job: workerJob({
          createdAt: "2026-07-17T13:17:16Z",
          startedAt: "2026-07-17T15:37:16Z",
        }),
        policy,
      }),
    (error) => {
      assert.ok(error instanceof DeadlineExceeded);
      assert.match(error.message, /waited 8400s for a runner slot/);
      assert.match(error.message, /7200s queue admission budget/);
      assert.match(error.message, /deadline 2026-07-17T15:17:16\.000Z/);
      assert.match(error.message, /dispatcher\s+slot stayed busy/);
      assert.match(error.message, /ix#7625/);
      return true;
    },
  );
});

test("queued run count reads total_count and fails closed on malformed bodies", async () => {
  const requests = [];
  const depth = await queuedRunCount({
    fetchImpl: async (url) => {
      requests.push(new URL(url));
      return new Response(
        JSON.stringify({ total_count: 21, workflow_runs: [{}] }),
        { status: 200 },
      );
    },
    repository: "indexable-inc/ix",
    token: "secret",
  });

  assert.equal(depth, 21);
  assert.equal(requests.length, 1);
  assert.equal(
    requests[0].pathname,
    "/repos/indexable-inc/ix/actions/runs",
  );
  assert.equal(requests[0].searchParams.get("status"), "queued");

  for (const body of [
    JSON.stringify({ workflow_runs: [] }),
    JSON.stringify({ total_count: "21" }),
    JSON.stringify({ total_count: -1 }),
    JSON.stringify([]),
  ]) {
    await assert.rejects(
      queuedRunCount({
        fetchImpl: async () => new Response(body, { status: 200 }),
        repository: "indexable-inc/ix",
        token: "secret",
      }),
      /malformed/,
    );
  }

  await assert.rejects(
    queuedRunCount({
      fetchImpl: async () => new Response("[]", { status: 500 }),
      repository: "indexable-inc/ix",
      token: "secret",
    }),
    /failed with 500/,
  );
});

test("late sibling fails even when another worker started on time", async () => {
  const policy = loadPolicy();
  const late = await currentWorkerJob({
    fetchImpl: async () =>
      response([
        workerJob({
          name: "lint-build",
          runnerName: "runner-timely",
          startedAt: "2026-07-15T10:00:30Z",
        }),
        workerJob({ startedAt: "2026-07-15T12:00:01Z" }),
      ]),
    repository: "indexable-inc/ix",
    runAttempt: 2,
    runId: 42,
    runnerName: "runner-42",
    token: "secret",
  });

  assert.throws(() => assertQueueAdmission({ job: late, policy }), DeadlineExceeded);
});

test("setup has its own 120 second allowance", () => {
  const policy = loadPolicy();
  assert.doesNotThrow(() =>
    assertSetupAllowance({
      nowMilliseconds: 120_000,
      policy,
      startedAtMilliseconds: 0,
    }),
  );
  assert.throws(
    () =>
      assertSetupAllowance({
        nowMilliseconds: 120_001,
        policy,
        startedAtMilliseconds: 0,
      }),
    DeadlineExceeded,
  );
});

test("validation starts with the complete tier allowance after setup", () => {
  const policy = loadPolicy();
  // The routine tier is measured per repository (see catalog/policy.json
  // notes); the extended tier is shared.
  assert.equal(
    validationSeconds({
      bigChange: false,
      policy,
      repository: "indexable-inc/ix",
    }),
    7_500,
  );
  assert.equal(
    validationSeconds({
      bigChange: false,
      policy,
      repository: "indexable-inc/index",
    }),
    2_400,
  );
  for (const repository of ["indexable-inc/ix", "indexable-inc/index"]) {
    assert.equal(
      validationSeconds({ bigChange: true, policy, repository }),
      10_800,
    );
  }
});

test("an unknown repository is a configuration error, not a default", () => {
  const policy = loadPolicy();
  // A typo in the `repository` input must not silently hand the run whichever
  // budget happens to be the catalog default.
  assert.throws(
    () =>
      validationSeconds({
        bigChange: false,
        policy,
        repository: "indexable-inc/typo",
      }),
    /no repository entry for indexable-inc\/typo/,
  );
});

test("budget kill message names the clock, the elapsed time, and the remedy", () => {
  const policy = loadPolicy();
  const routine = budgetExceededMessage({
    allowedSeconds: 300,
    bigChange: false,
    elapsedSeconds: 301,
    policy,
  });
  assert.match(routine, /routine_validation_seconds=300s exceeded after 301s/);
  assert.match(routine, /add the ci\/big-change label/);
  assert.match(routine, /extended_validation_seconds=10800s budget/);
  // ix#8688: three agents read a budget kill as a style violation because the
  // only red context they saw was `lint`. The message must deny that first.
  assert.match(routine, /wall-clock timeout, not a failed check/);
  assert.match(routine, /mirroring this timeout rather than a lint or build error/);
  // ENG-10273: the label reaches a fresh attempt only, so the message must not
  // let anyone believe `--failed` will pick it up.
  assert.match(routine, /re-run the WHOLE workflow/);
  assert.match(routine, /`gh run rerun --failed` reuses this attempt's published budget/);

  const extended = budgetExceededMessage({
    allowedSeconds: 10_800,
    bigChange: true,
    elapsedSeconds: 10_930,
    policy,
  });
  assert.match(extended, /extended_validation_seconds=10800s exceeded after 10930s/);
  assert.match(extended, /already held the extended budget/);
  assert.match(extended, /wall-clock timeout, not a failed check/);
});

test("budget verdicts fill only the outputs the script never published", async () => {
  const directory = await mkdtemp(join(tmpdir(), "ci-budget-worker-verdicts-"));
  try {
    const output = join(directory, "github-output");
    // Heredoc form: the delimiter line must count as a published output.
    await writeFile(output, "lint_result<<GHDELIM\npartial\nGHDELIM\n");

    assert.deepEqual(recordBudgetVerdicts({ githubOutputPath: output }), [
      "nix_result",
    ]);
    assert.equal(
      await readFile(output, "utf8"),
      "lint_result<<GHDELIM\npartial\nGHDELIM\nnix_result=budget-exceeded\n",
    );
    // A killed script's last write may lack its newline; the backfill must
    // start a fresh line instead of corrupting that entry.
    await writeFile(output, "lint_result=succ");
    assert.deepEqual(recordBudgetVerdicts({ githubOutputPath: output }), [
      "nix_result",
    ]);
    assert.equal(
      await readFile(output, "utf8"),
      "lint_result=succ\nnix_result=budget-exceeded\n",
    );
    // No GITHUB_OUTPUT (running outside Actions) publishes nothing.
    assert.deepEqual(recordBudgetVerdicts({ githubOutputPath: "" }), []);
  } finally {
    await rm(directory, { force: true, recursive: true });
  }
});

test("script arguments cross the JSON boundary as distinct values", async () => {
  const directory = await mkdtemp(join(tmpdir(), "ci-budget-worker-arguments-"));
  try {
    const script = join(directory, "arguments.sh");
    const output = join(directory, "arguments.json");
    await writeFile(
      script,
      'printf \'["%s","%s"]\\n\' "$1" "$2" >arguments.json\n',
    );

    const status = await runBudgetedScript({
      graceSeconds: 0.05,
      scriptArguments: parseArguments('["one value","two"]'),
      scriptPath: "arguments.sh",
      validationSeconds: 1,
      workspace: directory,
    });

    assert.equal(status, 0);
    assert.equal(await readFile(output, "utf8"), '["one value","two"]\n');
    assert.throws(() => parseArguments('["valid", 3]'), /array of strings/);
  } finally {
    await rm(directory, { force: true, recursive: true });
  }
});

test("timeout kills a TERM-resistant descendant after its leader exits", async () => {
  const directory = await mkdtemp(join(tmpdir(), "ci-budget-worker-"));
  try {
    const fixture = await processTreeFixture(directory);
    const status = await runBudgetedScript({
      graceSeconds: 0.05,
      scriptPath: "spawn-tree.sh",
      validationSeconds: 0.05,
      workspace: directory,
    });
    assert.equal(status, 124);
    await assertProcessGone(Number(await waitForFile(fixture.descendant)));
  } finally {
    await rm(directory, { force: true, recursive: true });
  }
});

test("timeout announces the budget kill and backfills mirror verdicts", async (t) => {
  const directory = await mkdtemp(join(tmpdir(), "ci-budget-worker-report-"));
  try {
    const script = join(directory, "gate.sh");
    const output = join(directory, "github-output");
    const summary = join(directory, "github-step-summary");
    // Publish one honest phase result, then outlive the clock, like
    // run-ci-phases.sh does when the Nix phase overruns after lint finished.
    await writeFile(
      script,
      [
        "#!/usr/bin/env bash",
        "set -euo pipefail",
        "printf 'lint_result=success\\n' >>github-output",
        "sleep 60",
        "",
      ].join("\n"),
    );
    await writeFile(output, "");
    await writeFile(summary, "");

    const logged = [];
    t.mock.method(console, "log", (line) => logged.push(line));
    const annotated = [];
    t.mock.method(console, "error", (line) => annotated.push(line));

    const policy = loadPolicy();
    const status = await runBudgetedScript({
      graceSeconds: 0.05,
      // Wired exactly as main() wires it, with the budget shrunk to test size.
      onBudgetExceeded: (expiry) =>
        reportBudgetExceeded({
          ...expiry,
          bigChange: false,
          githubOutputPath: output,
          policy,
          summaryPath: summary,
        }),
      scriptPath: "gate.sh",
      validationSeconds: 0.5,
      workspace: directory,
    });

    assert.equal(status, 124);
    const logText = logged.join("\n");
    assert.match(logText, /routine_validation_seconds=0\.5s exceeded after \d+s/);
    assert.match(logText, /published nix_result=budget-exceeded/);
    assert.match(
      annotated.join("\n"),
      /^::error title=CI budget exceeded \(timeout, not a lint failure\)::validation budget routine_validation_seconds=0\.5s .* the label will not take effect\.$/m,
    );
    // The job summary is the first surface a reader reaches from the checks
    // pane, and before this it said nothing at all about the kill.
    assert.match(
      await readFile(summary, "utf8"),
      /^## CI budget exceeded \(timeout, not a lint failure\)$/m,
    );
    // The honest lint verdict survives; only the killed phase reads as policy.
    assert.equal(
      await readFile(output, "utf8"),
      "lint_result=success\nnix_result=budget-exceeded\n",
    );
  } finally {
    await rm(directory, { force: true, recursive: true });
  }
});

test("external cancellation cleans the command group", async () => {
  const directory = await mkdtemp(join(tmpdir(), "ci-budget-worker-signal-"));
  try {
    const fixture = await processTreeFixture(directory);
    const workerUrl = pathToFileURL(join(import.meta.dirname, "worker.mjs")).href;
    const helper = join(directory, "helper.mjs");
    await writeFile(
      helper,
      [
        `import { runBudgetedScript } from ${JSON.stringify(workerUrl)};`,
        "const status = await runBudgetedScript({",
        "  graceSeconds: 0.05,",
        "  scriptPath: 'spawn-tree.sh',",
        "  validationSeconds: 60,",
        `  workspace: ${JSON.stringify(directory)},`,
        "});",
        "process.exit(status);",
        "",
      ].join("\n"),
    );
    const child = spawn(process.execPath, [helper], { stdio: "inherit" });
    const descendant = Number(await waitForFile(fixture.descendant));
    child.kill("SIGTERM");
    const result = await waitForExit(child);
    assert.deepEqual(result, { code: 143, signal: null });
    await assertProcessGone(descendant);
  } finally {
    await rm(directory, { force: true, recursive: true });
  }
});
