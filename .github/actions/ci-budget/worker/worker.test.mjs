import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { pathToFileURL } from "node:url";
import test from "node:test";

import {
  DeadlineExceeded,
  assertQueueAdmission,
  assertSetupAllowance,
  currentWorkerJob,
  loadPolicy,
  parseArguments,
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
  assert.equal(policy.queue_start_seconds, 300);
  assert.equal(policy.setup_allowance_seconds, 120);
  assert.equal(policy.routine_validation_seconds, 300);
  assert.equal(policy.extended_validation_seconds, 10_800);
  assert.equal(policy.termination_grace_seconds, 10);
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
  assert.doesNotThrow(() =>
    assertQueueAdmission({ job: workerJob(), policy }),
  );
  assert.throws(
    () =>
      assertQueueAdmission({
        job: workerJob({ startedAt: "2026-07-15T10:05:00.001Z" }),
        policy,
      }),
    DeadlineExceeded,
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
        workerJob({ startedAt: "2026-07-15T10:05:01Z" }),
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
  assert.equal(validationSeconds({ bigChange: false, policy }), 300);
  assert.equal(validationSeconds({ bigChange: true, policy }), 10_800);
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
