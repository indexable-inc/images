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
  path = resolve(actionDirectory, "..", "policy.json"),
) {
  const parsed = JSON.parse(readFileSync(path, "utf8"));
  const expected = [
    "extended_setup_allowance_seconds",
    "extended_validation_seconds",
    "standard_seconds",
    "termination_grace_seconds",
  ];
  if (
    parsed === null ||
    Array.isArray(parsed) ||
    typeof parsed !== "object" ||
    JSON.stringify(Object.keys(parsed).sort()) !== JSON.stringify(expected)
  ) {
    throw new Error(`CI budget policy keys must be ${expected.join(", ")}`);
  }
  return Object.fromEntries(
    expected.map((name) => [name, parsePositiveInteger(parsed[name], name)]),
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

export async function workflowRunCreatedAt({
  apiUrl = "https://api.github.com",
  fetchImpl = fetch,
  repository,
  runAttempt,
  runId,
  token,
}) {
  const url = `${apiUrl}/repos/${repositoryPath(repository)}/actions/runs/${runId}`;
  const response = await fetchImpl(url, {
    headers: {
      Accept: "application/vnd.github+json",
      Authorization: `Bearer ${token}`,
      "X-GitHub-Api-Version": "2022-11-28",
    },
  });
  const body = await response.text();
  if (!response.ok) {
    throw new Error(`GitHub workflow attempt request failed with ${response.status}`);
  }
  const run = JSON.parse(body);
  if (run.run_attempt !== runAttempt) {
    throw new Error(
      `GitHub workflow run is on attempt ${run.run_attempt}, expected ${runAttempt}`,
    );
  }
  if (typeof run.created_at !== "string") {
    throw new Error("GitHub workflow run has no created_at");
  }
  const createdAt = Date.parse(run.created_at);
  if (!Number.isFinite(createdAt)) {
    throw new Error("GitHub workflow run has an invalid created_at");
  }
  return createdAt;
}

export function validationSeconds({
  bigChange,
  createdAtMilliseconds,
  nowMilliseconds,
  policy,
}) {
  if (bigChange) return policy.extended_validation_seconds;
  const deadline = createdAtMilliseconds + policy.standard_seconds * 1000;
  const remaining =
    Math.floor((deadline - nowMilliseconds) / 1000) -
    policy.termination_grace_seconds;
  if (remaining <= 0) {
    throw new DeadlineExceeded(
      "no time remains for validation and process termination",
    );
  }
  return remaining;
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
  const bigChange = parseBoolean(input("big-change", { required: true }), "big-change");
  const createdAtMilliseconds = await workflowRunCreatedAt({
    apiUrl: process.env.GITHUB_API_URL,
    repository: input("repository", { required: true }),
    runAttempt: parsePositiveInteger(
      input("run-attempt", { required: true }),
      "run-attempt",
    ),
    runId: parsePositiveInteger(input("run-id", { required: true }), "run-id"),
    token: input("token", { required: true }),
  });
  const nowMilliseconds = Date.now();
  if (
    !bigChange &&
    nowMilliseconds >= createdAtMilliseconds + policy.standard_seconds * 1000
  ) {
    throw new DeadlineExceeded("worker started after the total deadline");
  }
  if (mode === "check") return 0;
  const scriptPath = input("script", { required: true });
  const allowedSeconds = validationSeconds({
    bigChange,
    createdAtMilliseconds,
    nowMilliseconds,
    policy,
  });
  return runBudgetedScript({
    graceSeconds: policy.termination_grace_seconds,
    scriptArguments: parseArguments(input("arguments") || "[]"),
    scriptPath,
    validationSeconds: allowedSeconds,
    workspace: process.env.GITHUB_WORKSPACE ?? "",
  });
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  try {
    process.exitCode = await main();
  } catch (error) {
    const title =
      error instanceof DeadlineExceeded
        ? "CI total deadline exceeded"
        : "ci-budget-worker";
    console.error(`::error title=${title}::${error.message}`);
    process.exitCode = error instanceof DeadlineExceeded ? 124 : 1;
  }
}
