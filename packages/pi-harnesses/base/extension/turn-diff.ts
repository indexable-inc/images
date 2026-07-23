import path from "node:path";
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import { parseStatusPaths } from "./lib/git-files.js";

// Track exactly which files the last agent run touched (git delta vs a
// baseline snapshot at agent_start, plus edit/write tool targets), toast a
// summary at agent_end, and offer /diff to inspect or open them.
//
// /diff        pick a changed file, open it in $PI_DIFF_EDITOR or $EDITOR
// /diff list   list the changed files
// /diff clear  reset tracking
//
// Adapted from davis7dotsh/my-pi-setup (MIT, (c) 2026 Benjamin Davis);
// editor made configurable instead of hardcoding Zed.
const commandName = "diff";

function getStringPath(input: unknown): string | undefined {
  if (!input || typeof input !== "object" || !("path" in input)) return undefined;
  return typeof (input as any).path === "string" ? (input as any).path : undefined;
}

function toAbsolute(cwd: string, filePath: string): string {
  return path.isAbsolute(filePath) ? path.normalize(filePath) : path.resolve(cwd, filePath);
}

function toRelative(cwd: string, filePath: string): string {
  const relative = path.relative(cwd, filePath);
  return relative && !relative.startsWith("..") && !path.isAbsolute(relative) ? relative : filePath;
}

async function getGitChangedFiles(pi: ExtensionAPI, cwd: string): Promise<Set<string>> {
  const result = await pi.exec("git", ["status", "--porcelain", "--untracked-files=all"], {
    cwd,
    timeout: 5000,
  });
  if (result.code !== 0) return new Set<string>();
  const paths = parseStatusPaths(result.stdout) as Set<string>;
  return new Set([...paths].map((p) => toAbsolute(cwd, p)));
}

function difference(current: Set<string>, baseline: Set<string>): Set<string> {
  return new Set([...current].filter((file) => !baseline.has(file)));
}

export default function (pi: ExtensionAPI) {
  let gitBaseline = new Set<string>();
  let changedFiles = new Set<string>();
  let toolTouchedFiles = new Set<string>();

  pi.on("agent_start", async (_event: any, ctx: any) => {
    toolTouchedFiles = new Set();
    changedFiles = new Set();
    gitBaseline = await getGitChangedFiles(pi, ctx.cwd);
  });

  pi.on("tool_result", (event: any, ctx: any) => {
    if (event.toolName !== "edit" && event.toolName !== "write") return;
    const filePath = getStringPath(event.input);
    if (!filePath) return;
    toolTouchedFiles.add(toAbsolute(ctx.cwd, filePath));
  });

  pi.on("agent_end", async (_event: any, ctx: any) => {
    const gitChanged = await getGitChangedFiles(pi, ctx.cwd);
    changedFiles = new Set([...difference(gitChanged, gitBaseline), ...toolTouchedFiles]);
    if (changedFiles.size > 0 && ctx.hasUI) {
      ctx.ui.notify(`${changedFiles.size} changed file(s). Run /${commandName} to view.`, "info");
    }
  });

  pi.registerCommand(commandName, {
    description: "Show files changed by the last agent run and open one in your editor",
    handler: async (args: string, ctx: any) => {
      await ctx.waitForIdle();

      const arg = (args ?? "").trim();
      if (arg === "clear") {
        changedFiles = new Set();
        toolTouchedFiles = new Set();
        gitBaseline = await getGitChangedFiles(pi, ctx.cwd);
        ctx.ui.notify("Cleared changed file list", "info");
        return;
      }

      const files = [...changedFiles].sort((a, b) =>
        toRelative(ctx.cwd, a).localeCompare(toRelative(ctx.cwd, b)),
      );
      if (files.length === 0) {
        ctx.ui.notify("No changed files tracked from the last agent run", "info");
        return;
      }

      if (arg === "list") {
        ctx.ui.notify(
          `Changed files:\n${files.map((file) => `- ${toRelative(ctx.cwd, file)}`).join("\n")}`,
          "info",
        );
        return;
      }

      if (arg) {
        ctx.ui.notify(
          `Unknown /${commandName} argument: ${arg}. Try /${commandName}, /${commandName} list, or /${commandName} clear.`,
          "warning",
        );
        return;
      }

      const labels = files.map((file) => toRelative(ctx.cwd, file));
      const selected = await ctx.ui.select("Open changed file", labels);
      if (!selected) return;

      const selectedIndex = labels.indexOf(selected);
      const file = files[selectedIndex];
      if (!file) return;

      const editor = process.env.PI_DIFF_EDITOR || process.env.EDITOR;
      if (!editor) {
        ctx.ui.notify(`Set PI_DIFF_EDITOR or EDITOR to open files. Path: ${file}`, "warning");
        return;
      }

      const result = await pi.exec(editor, [file], { cwd: ctx.cwd, timeout: 10000 });
      if (result.code === 0) {
        ctx.ui.notify(`Opened ${selected}`, "info");
      } else {
        ctx.ui.notify(result.stderr.trim() || `Failed to open ${selected} with ${editor}`, "error");
      }
    },
  });
}
