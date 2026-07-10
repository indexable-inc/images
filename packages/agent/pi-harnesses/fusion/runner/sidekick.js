import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { countDiffLines, runProcess as sh } from "./process.js";

export function buildSidekickPrompt({ goal, task, acceptance, mode }) {
  const parts = [
    "Goal: " + (goal || "(not captured)"),
    "",
    "Delegated task: " + task,
    "",
    "You are the sidekick worker in a fusion harness. Do the delegated work, gather only the context you need, and keep the primary agent's context clean.",
    "Prefer small, directly relevant changes. Run cheap verification when possible.",
  ];
  if (acceptance) {
    parts.push("", "Acceptance check or success criteria: " + acceptance);
  }
  if (mode === "inspect") {
    parts.push("", "Inspection mode: do not edit files. Return findings, risks, and the exact files or commands the primary should inspect.");
  } else {
    parts.push("", "Edit mode: make the necessary file changes. End with a concise summary and the commands you ran.");
  }
  return parts.join("\n");
}

export { countDiffLines };

export function formatSidekickResult(result) {
  const status = result.exitCode === 0 ? "completed" : "failed (exit " + result.exitCode + ")";
  const patch = result.patch?.trim() ? result.patch : "(no file changes produced)";
  const summary = result.stdout?.trim() ? result.stdout.trim().slice(-4000) : "(no sidekick stdout)";
  const stderr = result.stderr?.trim() ? "\n\n--- sidekick stderr ---\n" + result.stderr.trim().slice(-2000) : "";
  return (
    "Sidekick " + status + ". Diff lines: " + result.diffLines + ".\n\n" +
    "--- sidekick summary ---\n" + summary + stderr + "\n\n" +
    "--- sidekick patch ---\n" + patch
  );
}

export async function runSidekick({
  goal,
  task,
  acceptance,
  mode = "edit",
  repoRoot,
  provider,
  model,
  thinking,
  timeoutSec = 300,
  isolatedWorktree = true,
}) {
  const cwd = repoRoot;
  const prompt = buildSidekickPrompt({ goal, task, acceptance, mode });
  let worktree = cwd;
  let tempDir;
  let addedWorktree = false;

  if (isolatedWorktree) {
    tempDir = await mkdtemp(join(tmpdir(), "fusion-sidekick-"));
    const added = await sh("git", ["-C", cwd, "worktree", "add", "-q", "--detach", tempDir, "HEAD"]);
    if (added.code !== 0) {
      return {
        exitCode: added.code,
        stdout: "",
        stderr: "worktree add failed: " + added.stderr,
        diffLines: 0,
        patch: "",
        prompt,
      };
    }
    worktree = tempDir;
    addedWorktree = true;
  }

  try {
    const piArgs = ["--print"];
    if (provider) piArgs.push("--provider", provider);
    if (model) piArgs.push("--model", model);
    if (thinking) piArgs.push("--thinking", thinking);
    piArgs.push(prompt);

    const sidekick = await sh("timeout", [String(timeoutSec) + "s", "pi", ...piArgs], {
      cwd: worktree,
      env: process.env,
    });
    const diff = await sh("git", ["-C", worktree, "diff", "--no-color"]);
    const numstat = await sh("git", ["-C", worktree, "diff", "--numstat"]);

    return {
      exitCode: sidekick.code,
      stdout: sidekick.stdout,
      stderr: sidekick.stderr,
      diffLines: countDiffLines(numstat.stdout),
      patch: diff.stdout,
      prompt,
    };
  } finally {
    if (addedWorktree) {
      await sh("git", ["-C", cwd, "worktree", "remove", "--force", worktree]);
    }
    if (tempDir) {
      await rm(tempDir, { recursive: true, force: true }).catch(() => {});
    }
  }
}
