import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";

const delegateSchema = {
  type: "object",
  properties: {
    task: {
      type: "string",
      description: "The concrete work to delegate to the sidekick.",
    },
    acceptance: {
      type: "string",
      description: "Optional check, command, or success criteria for the sidekick.",
    },
    mode: {
      type: "string",
      enum: ["edit", "inspect"],
      default: "edit",
      description: "Use inspect for research-only delegation; edit allows file changes in the sidekick worktree.",
    },
    timeoutSec: {
      type: "integer",
      default: 300,
      description: "Hard wall-clock cap for the sidekick run.",
    },
  },
  required: ["task"],
  additionalProperties: false,
} as const;

function sidekickSelection() {
  const alias = process.env.PI_FUSION_SIDEKICK_ALIAS ?? "codex-low";
  return {
    alias,
    provider: process.env.PI_FUSION_SIDEKICK_PROVIDER ?? "openai",
    model: process.env.PI_FUSION_SIDEKICK_MODEL ?? "gpt-5.5",
    thinking: process.env.PI_FUSION_SIDEKICK_THINKING ?? "low",
  };
}

export default function (pi: ExtensionAPI): void {
  let goal = process.env.PI_FUSION_GOAL ?? "";

  pi.on("agent_start", (event: any) => {
    if (goal) return;
    const messages = event?.messages;
    if (!Array.isArray(messages)) return;
    const first = messages.find((m: any) => m?.role === "user");
    if (first && typeof first.content === "string") goal = first.content;
  });

  const updateStatus = (ctx: any) => {
    if (!ctx?.hasUI) return;
    const theme = ctx.ui.theme;
    const sidekick = sidekickSelection();
    ctx.ui.setStatus(
      "fusion",
      theme.fg("accent", "fusion") + " " + theme.fg("dim", "sidekick " + sidekick.alias),
    );
  };

  pi.on("session_start", (_event: any, ctx: any) => updateStatus(ctx));

  pi.registerTool({
    name: "delegate",
    description:
      "Delegate bounded work to the fusion sidekick. The primary agent should use this for bulk edits, broad inspection, and test loops, then review the returned summary and patch before applying anything.",
    parameters: delegateSchema as never,
    async execute(_toolCallId: string, params: any, _signal: unknown, _onUpdate: unknown, ctx: any) {
      if (ctx?.hasUI) ctx.ui.setWorkingMessage("fusion sidekick working...");
      const root = await pi
        .exec("git", ["rev-parse", "--show-toplevel"], { timeout: 10000 })
        .then((r: any) => r.stdout.trim())
        .catch(() => "");
      const repoRoot = root || ctx?.cwd;
      const sidekick = sidekickSelection();

      try {
        const { runSidekick, formatSidekickResult } = await import("./lib/sidekick.js");
        const result = await runSidekick({
          goal,
          task: params.task,
          acceptance: params.acceptance,
          mode: params.mode ?? "edit",
          repoRoot,
          provider: sidekick.provider,
          model: sidekick.model,
          thinking: sidekick.thinking,
          timeoutSec: params.timeoutSec ?? 300,
          isolatedWorktree: true,
        });

        pi.appendEntry("fusion", {
          goal,
          task: params.task,
          acceptance: params.acceptance,
          mode: params.mode ?? "edit",
          sidekick,
          exitCode: result.exitCode,
          diffLines: result.diffLines,
        });

        if (ctx?.hasUI) {
          const level = result.exitCode === 0 ? "info" : "warning";
          ctx.ui.notify("Sidekick returned " + result.diffLines + " diff line(s)", level);
        }

        return {
          content: [{ type: "text", text: formatSidekickResult(result) }],
          details: { isError: result.exitCode !== 0 },
        };
      } finally {
        if (ctx?.hasUI) ctx.ui.setWorkingMessage(undefined);
        updateStatus(ctx);
      }
    },
  });
}
