import { spawn } from "node:child_process";
import { readFileSync } from "node:fs";
import { constants as osConstants } from "node:os";
import { dirname, isAbsolute, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const actionDirectory = dirname(fileURLToPath(import.meta.url));

export class DeadlineExceeded extends Error {
  constructor(message) {
    super(message);
    this.name = "DeadlineExceeded";
  }
}

function input(name, { required = false } = {}) {
  const key = `INPUT_${name.toUpperCase().replaceAll(" ", "_")}`;
  const value = process.env[key]?.trim() ?? "";
  if (required && value === "") {
    throw new Error(`input ${name} is required`);
  }
  return value;
}

export function parsePositiveInteger(value, name) {
  if (!/^[1-9][0-9]*$/.test(String(value))) {
    throw new Error(`${name} must be a positive integer`);
  }
  const parsed = Number(value);
  if (!Number.isSafeInteger(parsed)) {
    throw new Error(`${name} exceeds the safe integer range`);
  }
  return parsed;
}

export function parseBoolean(value, name) {
  if (value === "true") return true;
  if (value === "false") return false;
  throw new Error(`${name} must be true or false`);
}

export function parseArguments(value) {
  const parsed = JSON.parse(value);
  if (!Array.isArray(parsed) || !parsed.every((item) => typeof item === "string")) {
    throw new Error("arguments must be a JSON array of strings");
  }
  return parsed;
}

export function loadPolicy(
  path = resolve(actionDirectory, "..", "catalog", "policy.json"),
) {
  const parsed = JSON.parse(readFileSync(path, "utf8"));
  const numeric = [
    "extended_validation_seconds",
    "queue_start_seconds",
    "routine_validation_seconds",
    "setup_allowance_seconds",
    "termination_grace_seconds",
  ];
  const expected = [
    "big_change_label",
    "extended_validation_seconds",
    "queue_start_seconds",
    "repositories",
    "routine_validation_seconds",
    "setup_allowance_seconds",
    "termination_grace_seconds",
  ];
  if (
    parsed === null ||
    Array.isArray(parsed) ||
    typeof parsed !== "object" ||
    JSON.stringify(Object.keys(parsed).sort()) !== JSON.stringify(expected) ||
    typeof parsed.big_change_label !== "string" ||
    parsed.big_change_label === "" ||
    parsed.repositories === null ||
    Array.isArray(parsed.repositories) ||
    typeof parsed.repositories !== "object"
  ) {
    throw new Error(`CI budget policy keys must be ${expected.join(", ")}`);
  }
  return Object.fromEntries(
    numeric.map((name) => [name, parsePositiveInteger(parsed[name], name)]),
  );
}

function repositoryPath(repository) {
  const parts = repository.split("/");
  if (
    parts.length !== 2 ||
    parts.some((part) => !/^[A-Za-z0-9_.-]+$/.test(part))
  ) {
    throw new Error("repository must use owner/name form");
  }
  return parts.map(encodeURIComponent).join("/");
}

function workflowJobsUrl({
  apiUrl,
  page,
  repository,
  runAttempt,
  runId,
}) {
  const url = new URL(apiUrl);
  const basePath = url.pathname.replace(/\/$/, "");
  url.pathname = `${basePath}/repos/${repositoryPath(repository)}/actions/runs/${runId}/attempts/${runAttempt}/jobs`;
  url.searchParams.set("per_page", "100");
  url.searchParams.set("page", String(page));
  return url;
}

async function workflowJobs({
  apiUrl = "https://api.github.com",
  fetchImpl = fetch,
  repository,
  runAttempt,
  runId,
  token,
}) {
  const jobs = [];
  for (let page = 1; ; page += 1) {
    const url = workflowJobsUrl({
      apiUrl,
      page,
      repository,
      runAttempt,
      runId,
    });
    const response = await fetchImpl(url, {
      headers: {
        Accept: "application/vnd.github+json",
        Authorization: `Bearer ${token}`,
        "X-GitHub-Api-Version": "2022-11-28",
      },
    });
    const body = await response.text();
    if (!response.ok) {
      throw new Error(`GitHub workflow jobs request failed with ${response.status}`);
    }
    const payload = JSON.parse(body);
    if (
      payload === null ||
      Array.isArray(payload) ||
      typeof payload !== "object" ||
      !Array.isArray(payload.jobs) ||
      !payload.jobs.every(
        (job) => job !== null && !Array.isArray(job) && typeof job === "object",
      )
    ) {
      throw new Error("GitHub workflow jobs response is malformed");
    }
    jobs.push(...payload.jobs);
    if (payload.jobs.length < 100) return jobs;
  }
}

export async function currentWorkerJob({
  apiUrl = "https://api.github.com",
  fetchImpl = fetch,
  repository,
  runAttempt,
  runId,
  runnerName,
  token,
}) {
  if (typeof runnerName !== "string" || runnerName === "") {
    throw new Error("RUNNER_NAME is required");
  }
  const jobs = await workflowJobs({
    apiUrl,
    fetchImpl,
    repository,
    runAttempt,
    runId,
    token,
  });
  // An ephemeral runner executes one job. Its RUNNER_NAME is the same exact
  // value exposed by the attempt jobs endpoint, so no workflow job-name guess
  // or matrix-name serialization is needed.
  const matches = jobs.filter(
    (job) => job.runner_name === runnerName && job.status === "in_progress",
  );
  if (matches.length !== 1) {
    throw new Error(
      `workflow attempt has ${matches.length} active workers on runner ${JSON.stringify(runnerName)}; expected exactly one`,
    );
  }
  const [job] = matches;
  if (job.run_attempt !== runAttempt) {
    throw new Error(
      `GitHub worker job has attempt ${job.run_attempt}, expected ${runAttempt}`,
    );
  }
  return job;
}

/// Best-effort queue-depth probe for the admission failure message: how many
/// workflow runs the repository still has queued while this late worker runs
/// its check. The count is diagnostic context only, so callers must not let a
/// failure here mask the deadline miss itself.
export async function queuedRunCount({
  apiUrl = "https://api.github.com",
  fetchImpl = fetch,
  repository,
  token,
}) {
  const url = new URL(apiUrl);
  const basePath = url.pathname.replace(/\/$/, "");
  url.pathname = `${basePath}/repos/${repositoryPath(repository)}/actions/runs`;
  url.searchParams.set("status", "queued");
  url.searchParams.set("per_page", "1");
  const response = await fetchImpl(url, {
    headers: {
      Accept: "application/vnd.github+json",
      Authorization: `Bearer ${token}`,
      "X-GitHub-Api-Version": "2022-11-28",
    },
  });
  const body = await response.text();
  if (!response.ok) {
    throw new Error(`GitHub queued runs request failed with ${response.status}`);
  }
  const payload = JSON.parse(body);
  if (
    payload === null ||
    Array.isArray(payload) ||
    typeof payload !== "object" ||
    typeof payload.total_count !== "number" ||
    !Number.isSafeInteger(payload.total_count) ||
    payload.total_count < 0
  ) {
    throw new Error("GitHub queued runs response is malformed");
  }
  return payload.total_count;
}

function timestamp(value, name) {
  if (typeof value !== "string") {
    throw new Error(`GitHub worker job has no ${name}`);
  }
  const milliseconds = Date.parse(value);
  if (!Number.isFinite(milliseconds)) {
    throw new Error(`GitHub worker job has invalid ${name}`);
  }
  return milliseconds;
}

export function workerTiming(job) {
  return {
    createdAtMilliseconds: timestamp(job.created_at, "created_at"),
    startedAtMilliseconds: timestamp(job.started_at, "started_at"),
  };
}

export function assertQueueAdmission({ job, policy }) {
  const timing = workerTiming(job);
  const deadline =
    timing.createdAtMilliseconds + policy.queue_start_seconds * 1000;
  if (timing.startedAtMilliseconds > deadline) {
    // A late start means no dispatcher slot freed up in time, not that this
    // runner was slow: the self-hosted pool is one slot per host, shared by
    // every workflow (PR ci, auto-deploy, prod deploy, cache-warm, publish,
    // antithesis, ...), so a stacked deploy backlog starves PR admission
    // (indexable-inc/ix#7625). Say how long the job actually waited so the
    // failure reads as pool saturation instead of a runner defect.
    const waitedSeconds = Math.round(
      (timing.startedAtMilliseconds - timing.createdAtMilliseconds) / 1000,
    );
    throw new DeadlineExceeded(
      `worker waited ${waitedSeconds}s for a runner slot, over the ` +
        `${policy.queue_start_seconds}s queue admission budget (created ` +
        `${job.created_at}, started ${job.started_at}, deadline ` +
        `${new Date(deadline).toISOString()}). Every self-hosted dispatcher ` +
        `slot stayed busy past the deadline; rerun once the backlog drains ` +
        `(indexable-inc/ix#7625)`,
    );
  }
  return timing;
}

export function assertSetupAllowance({
  nowMilliseconds,
  policy,
  startedAtMilliseconds,
}) {
  const deadline =
    startedAtMilliseconds + policy.setup_allowance_seconds * 1000;
  if (nowMilliseconds > deadline) {
    throw new DeadlineExceeded(
      `worker setup exceeded ${policy.setup_allowance_seconds} seconds`,
    );
  }
}

export function validationSeconds({ bigChange, policy }) {
  if (bigChange) return policy.extended_validation_seconds;
  return policy.routine_validation_seconds;
}

function groupExists(pid) {
  try {
    process.kill(-pid, 0);
    return true;
  } catch (error) {
    if (error?.code === "ESRCH") return false;
    throw error;
  }
}

function signalGroup(pid, signal) {
  try {
    process.kill(-pid, signal);
    return true;
  } catch (error) {
    if (error?.code === "ESRCH") return false;
    throw error;
  }
}

const delay = (milliseconds) =>
  new Promise((resolveDelay) => setTimeout(resolveDelay, milliseconds));

export async function terminateGroup(pid, graceMilliseconds) {
  if (!signalGroup(pid, "SIGTERM")) return;
  const startedAt = Date.now();
  const killWaitMilliseconds = Math.min(1000, graceMilliseconds / 2);
  const termDeadline = startedAt + graceMilliseconds - killWaitMilliseconds;
  while (groupExists(pid) && Date.now() < termDeadline) {
    await delay(Math.min(50, Math.max(1, termDeadline - Date.now())));
  }
  if (!groupExists(pid)) return;
  signalGroup(pid, "SIGKILL");
  const cleanupDeadline = startedAt + graceMilliseconds;
  while (groupExists(pid) && Date.now() < cleanupDeadline) {
    await delay(Math.min(25, Math.max(1, cleanupDeadline - Date.now())));
  }
  if (groupExists(pid)) {
    throw new Error(`process group ${pid} survived SIGKILL cleanup`);
  }
}

function signalExitCode(signal) {
  const number = osConstants.signals[signal];
  return typeof number === "number" ? 128 + number : 1;
}

export async function runBudgetedScript({
  graceSeconds,
  scriptArguments = [],
  scriptPath,
  validationSeconds: allowedSeconds,
  workspace,
}) {
  const absoluteScript = resolve(workspace, scriptPath);
  if (
    isAbsolute(scriptPath) ||
    relative(workspace, absoluteScript).startsWith("..")
  ) {
    throw new Error("script must stay inside GITHUB_WORKSPACE");
  }

  const child = spawn("bash", [absoluteScript, ...scriptArguments], {
    cwd: workspace,
    detached: true,
    env: process.env,
    stdio: "inherit",
  });
  if (child.pid === undefined) throw new Error("failed to start budgeted script");

  const childDone = new Promise((resolveChild, rejectChild) => {
    child.once("error", rejectChild);
    child.once("exit", (code, signal) =>
      resolveChild({ kind: "child", code, signal }),
    );
  });
  let timeout;
  const timedOut = new Promise((resolveTimeout) => {
    timeout = setTimeout(
      () => resolveTimeout({ kind: "timeout" }),
      allowedSeconds * 1000,
    );
  });
  let resolveExternalSignal;
  const externalSignal = new Promise((resolveSignal) => {
    resolveExternalSignal = resolveSignal;
  });
  const handledSignals = ["SIGHUP", "SIGINT", "SIGTERM"];
  const handlers = new Map(
    handledSignals.map((signal) => [
      signal,
      () => resolveExternalSignal({ kind: "signal", signal }),
    ]),
  );
  for (const [signal, handler] of handlers) process.once(signal, handler);

  let result;
  try {
    result = await Promise.race([childDone, timedOut, externalSignal]);
    clearTimeout(timeout);
    await terminateGroup(child.pid, graceSeconds * 1000);
    if (result.kind !== "child") {
      await Promise.race([childDone.catch(() => undefined), delay(1000)]);
    }
  } finally {
    clearTimeout(timeout);
    for (const [signal, handler] of handlers) {
      process.removeListener(signal, handler);
    }
  }

  if (result.kind === "timeout") return 124;
  if (result.kind === "signal") return signalExitCode(result.signal);
  if (result.code !== null) return result.code;
  return signalExitCode(result.signal);
}

export async function main() {
  const mode = input("mode") || "check";
  if (!new Set(["check", "run"]).has(mode)) {
    throw new Error(`unknown worker mode ${mode}`);
  }
  const policy = loadPolicy();
  const bigChange = parseBoolean(
    input("big-change", { required: true }),
    "big-change",
  );
  const repository = input("repository", { required: true });
  const runAttempt = parsePositiveInteger(
    input("run-attempt", { required: true }),
    "run-attempt",
  );
  const runId = parsePositiveInteger(
    input("run-id", { required: true }),
    "run-id",
  );
  const token = input("token", { required: true });
  const job = await currentWorkerJob({
    apiUrl: process.env.GITHUB_API_URL,
    repository,
    runAttempt,
    runId,
    runnerName: process.env.RUNNER_NAME,
    token,
  });
  let timing;
  try {
    timing = assertQueueAdmission({ job, policy });
  } catch (error) {
    if (error instanceof DeadlineExceeded) {
      try {
        const depth = await queuedRunCount({
          apiUrl: process.env.GITHUB_API_URL,
          repository,
          token,
        });
        error.message += ` ${depth} workflow run(s) were still queued in ${repository} at check time.`;
      } catch {
        // Queue depth is best-effort context; a second GitHub API failure
        // must not mask the deadline miss itself.
      }
    }
    throw error;
  }
  if (mode === "check") return 0;
  assertSetupAllowance({
    nowMilliseconds: Date.now(),
    policy,
    startedAtMilliseconds: timing.startedAtMilliseconds,
  });
  const scriptPath = input("script", { required: true });
  const allowedSeconds = validationSeconds({ bigChange, policy });
  return runBudgetedScript({
    graceSeconds: policy.termination_grace_seconds,
    scriptArguments: parseArguments(input("arguments") || "[]"),
    scriptPath,
    validationSeconds: allowedSeconds,
    workspace: process.env.GITHUB_WORKSPACE ?? "",
  });
}

export async function runAsAction() {
  try {
    process.exitCode = await main();
  } catch (error) {
    const title =
      error instanceof DeadlineExceeded
        ? "CI worker budget exceeded"
        : "ci-budget-worker";
    console.error(`::error title=${title}::${error.message}`);
    process.exitCode = error instanceof DeadlineExceeded ? 124 : 1;
  }
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  await runAsAction();
}
