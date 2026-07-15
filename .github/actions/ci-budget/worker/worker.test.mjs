import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { pathToFileURL } from "node:url";
import test from "node:test";

import {
  DeadlineExceeded,
  loadPolicy,
  parseArguments,
  runBudgetedScript,
  validationSeconds,
  workflowRunCreatedAt,
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
      "printf '%s\\n' \"$!\" >descendant-pid",
      "wait",
      "",
    ].join("\n"),
  );
  return { descendant, script };
}

test("policy is shared with the classifier", () => {
  const policy = loadPolicy();
  assert.equal(policy.standard_seconds, 300);
  assert.equal(policy.extended_validation_seconds, 10_800);
  assert.equal(policy.termination_grace_seconds, 10);
});

test("ordinary retries keep the original workflow creation deadline", async () => {
  let requestedUrl;
  const createdAt = await workflowRunCreatedAt({
    apiUrl: "https://api.example.test",
    fetchImpl: async (url) => {
      requestedUrl = url;
      return new Response(
        JSON.stringify({
          created_at: "2026-07-15T12:00:00Z",
          run_attempt: 2,
          run_started_at: "2026-07-15T13:00:00Z",
        }),
        { status: 200 },
      );
    },
    repository: "indexable-inc/ix",
    runAttempt: 2,
    runId: 42,
    token: "secret",
  });
  assert.equal(
    requestedUrl,
    "https://api.example.test/repos/indexable-inc/ix/actions/runs/42",
  );
  assert.equal(createdAt, Date.parse("2026-07-15T12:00:00Z"));
});

test("worker rejects a different current retry attempt", async () => {
  await assert.rejects(
    workflowRunCreatedAt({
      apiUrl: "https://api.example.test",
      fetchImpl: async () =>
        new Response(
          JSON.stringify({
            created_at: "2026-07-15T12:00:00Z",
            run_attempt: 3,
          }),
          { status: 200 },
        ),
      repository: "indexable-inc/ix",
      runAttempt: 2,
      runId: 42,
      token: "secret",
    }),
    /attempt 3, expected 2/,
  );
});

test("routine validation consumes only the workflow creation remainder", () => {
  const policy = loadPolicy();
  assert.equal(
    validationSeconds({
      bigChange: false,
      createdAtMilliseconds: 0,
      nowMilliseconds: 289_000,
      policy,
    }),
    1,
  );
});

test("retry after the workflow deadline fails before starting work", () => {
  const policy = loadPolicy();
  assert.throws(
    () =>
      validationSeconds({
        bigChange: false,
        createdAtMilliseconds: 0,
        nowMilliseconds: 291_001,
        policy,
      }),
    DeadlineExceeded,
  );
});

test("script arguments cross the JSON boundary as distinct values", async () => {
  const directory = await mkdtemp(join(tmpdir(), "ci-budget-worker-arguments-"));
  try {
    const script = join(directory, "arguments.sh");
    const output = join(directory, "arguments.json");
    await writeFile(
      script,
      "printf '[\"%s\",\"%s\"]\\n' \"$1\" \"$2\" >arguments.json\n",
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
