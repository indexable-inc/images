import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import { execFile } from "node:child_process";
import { promisify } from "node:util";
import { countUnstagedFiles } from "./lib/git-files.js";

// Persistent widget above the editor showing the current branch and unstaged
// file count, refreshed on a timer plus input/tool activity.
//
// Adapted from davis7dotsh/my-pi-setup (MIT, (c) 2026 Benjamin Davis).
const execFileAsync = promisify(execFile);
const WIDGET_ID = "git-status-widget";
const UPDATE_INTERVAL_MS = 2_000;

async function runGit(args: string[], cwd: string): Promise<string> {
  const { stdout } = await execFileAsync("git", args, {
    cwd,
    timeout: 2_000,
    maxBuffer: 1024 * 1024,
  });
  return stdout.trimEnd();
}

async function getBranch(cwd: string): Promise<string> {
  const branch = await runGit(["branch", "--show-current"], cwd);
  if (branch.length > 0) return branch;
  const head = await runGit(["rev-parse", "--short", "HEAD"], cwd);
  return head.length > 0 ? `detached@${head}` : "unknown";
}

async function updateWidget(ctx: any): Promise<void> {
  if (!ctx.hasUI) return;
  try {
    await runGit(["rev-parse", "--is-inside-work-tree"], ctx.cwd);
    const [branch, status] = await Promise.all([
      getBranch(ctx.cwd),
      runGit(["status", "--porcelain", "--untracked-files=normal"], ctx.cwd),
    ]);
    const unstagedCount = countUnstagedFiles(status);
    const fileLabel = unstagedCount === 1 ? "file" : "files";
    ctx.ui.setWidget(WIDGET_ID, [` ${branch} · ${unstagedCount} unstaged ${fileLabel}`]);
  } catch {
    ctx.ui.setWidget(WIDGET_ID, undefined);
  }
}

export default function (pi: ExtensionAPI) {
  let interval: ReturnType<typeof setInterval> | undefined;

  pi.on("session_start", async (_event: any, ctx: any) => {
    if (interval) clearInterval(interval);
    await updateWidget(ctx);
    interval = setInterval(() => {
      void updateWidget(ctx);
    }, UPDATE_INTERVAL_MS);
  });

  pi.on("input", async (_event: any, ctx: any) => {
    await updateWidget(ctx);
    return { action: "continue" as const };
  });

  pi.on("tool_execution_end", async (_event: any, ctx: any) => {
    await updateWidget(ctx);
  });

  pi.on("session_shutdown", async (_event: any, ctx: any) => {
    if (interval) {
      clearInterval(interval);
      interval = undefined;
    }
    if (ctx.hasUI) ctx.ui.setWidget(WIDGET_ID, undefined);
  });
}
