import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";

// Beam-search harness: turn a hard decision into a bounded search instead of a
// linear commitment.
//
// The executor calls explore({approaches, score}). Each approach runs on its
// own isolated git worktree under a turn + wall-clock budget; branches are
// scored on GROUND TRUTH (the score command's exit code, then diff size) and
// returned ranked. The executor then applies the winning patch itself - beam
// proposes, the executor commits. Dead ends die in a few turns instead of after
// forty.

const exploreSchema = {
  type: "object",
  properties: {
    approaches: {
      type: "array",
      items: { type: "string" },
      minItems: 2,
      maxItems: 4,
      description: "2-4 distinct approaches to the current decision.",
    },
    score: {
      type: "string",
      description:
        "Shell command whose exit code (0 = pass) and resulting diff size rank a " +
        "branch, e.g. 'cargo check' or 'npm test'. Optional; omit to rank on diff size only.",
    },
    turnCap: {
      type: "integer",
      default: 6,
      description: "Max model turns per branch (soft cap).",
    },
    timeoutSec: {
      type: "integer",
      default: 180,
      description: "Hard wall-clock cap per branch in seconds.",
    },
  },
  required: ["approaches"],
  additionalProperties: false,
} as const;

export default function (pi: ExtensionAPI): void {
  let goal = process.env.PI_BEAM_GOAL ?? "";
  pi.on("agent_start", (event: any) => {
    if (goal) return;
    const messages = event?.messages;
    if (!Array.isArray(messages)) return;
    const first = messages.find((m: any) => m?.role === "user");
    if (first && typeof first.content === "string") goal = first.content;
  });

  pi.registerTool({
    name: "explore",
    description:
      "Propose 2-4 approaches to the current decision. Each runs on an isolated git " +
      "worktree under a turn + time budget; branches are scored on ground truth (your " +
      "score command's exit code, then diff size) and returned ranked. Apply the winning " +
      "patch yourself to proceed.",
    parameters: exploreSchema as never,
    async execute(_toolCallId: string, params: any, _signal: unknown, _onUpdate: unknown, ctx: any) {
      const root = await pi
        .exec("git", ["rev-parse", "--show-toplevel"], { timeout: 10000 })
        .then((r: any) => r.stdout.trim())
        .catch(() => "");
      const repoRoot = root || ctx?.cwd;

      const { fanout } = await import("./lib/fanout.js");
      const ranked = await fanout({
        approaches: params.approaches,
        goal,
        repoRoot,
        scoreCmd: params.score,
        provider: process.env.PI_PROVIDER,
        model: process.env.PI_MODEL,
        thinking: process.env.PI_THINKING,
        turnCap: params.turnCap ?? 6,
        timeoutSec: params.timeoutSec ?? 180,
      });

      pi.appendEntry("beam", {
        goal,
        approaches: params.approaches,
        ranked: ranked.map((r: any) => ({
          approach: r.approach,
          exitCode: r.exitCode,
          diffLines: r.diffLines,
          score: r.score,
        })),
      });

      const winner = ranked[0];
      const table = ranked
        .map(
          (r: any, i: number) =>
            `${i === 0 ? "WINNER" : "      "} [${r.exitCode === 0 ? "pass" : "FAIL"} diff:${r.diffLines}] ${r.approach}`,
        )
        .join("\n");
      const patch = winner?.patch?.trim() ? winner.patch : "(no file changes produced)";

      return {
        content: [
          {
            type: "text",
            text:
              `Explored ${ranked.length} approaches under budget.\n${table}\n\n` +
              `--- winning patch (apply it to proceed) ---\n${patch}`,
          },
        ],
        details: { isError: false },
      };
    },
  });
}
