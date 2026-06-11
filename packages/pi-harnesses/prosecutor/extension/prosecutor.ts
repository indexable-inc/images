import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import { runIsolatedPi } from "./lib/child-agent.js";
import { parseVerdict } from "./lib/probes.js";
import { createTrust } from "./lib/trust.js";

// Prosecutor harness: an executor under a skeptical supervisor with
// earned-trust check-ins.
//
//   - The executor does normal work with its full tool surface.
//   - On a deterministic-but-adaptive interval it is forced to STOP and state
//     one falsifiable claim via claim({statement, verify}).
//   - That claim is handed to an isolated prosecutor (a fresh `pi` process with
//     NO access to the executor's transcript) which probes ground truth and
//     returns UPHELD or BROKEN.
//   - Trust is earned: an upheld claim widens the interval, a broken one snaps
//     it back to every action.
//
// No shared context means the two agents cannot launder each other's
// hallucinations - the supervisor only ever sees the claim and the repo.

// Plain JSON Schema. Pi accepts a JSON-Schema-shaped object for `parameters`
// (TypeBox is JSON Schema at runtime), so we need no schema dependency.
const claimSchema = {
  type: "object",
  properties: {
    statement: {
      type: "string",
      description: "ONE falsifiable claim about current progress.",
    },
    verify: {
      type: "string",
      description: "A shell command that, if it exits 0, supports the claim.",
    },
  },
  required: ["statement", "verify"],
  additionalProperties: false,
} as const;

export default function (pi: ExtensionAPI): void {
  const trust = createTrust({ min: 1, max: 16 });
  let sinceCheckin = 0;
  let goal = process.env.PI_PROSECUTOR_GOAL ?? "";

  // Capture the objective from the first user message if not set via env, so
  // the prosecutor can judge the claim against what the user actually asked.
  pi.on("agent_start", (event: any) => {
    if (goal) return;
    const messages = event?.messages;
    if (!Array.isArray(messages)) return;
    const first = messages.find((m: any) => m?.role === "user");
    if (first && typeof first.content === "string") goal = first.content;
  });

  // Adaptive interval gate: once the budget is spent, only `claim` may run.
  pi.on("tool_call", (event: any) => {
    if (event?.toolName === "claim") return undefined;
    if (sinceCheckin >= trust.interval) {
      return {
        block: true,
        reason:
          `Check-in due (interval=${trust.interval}). State your single most ` +
          `important falsifiable claim via claim({statement, verify}) before any other tool.`,
      };
    }
    return undefined;
  });

  // Keep the supervision state visible: a footer status with the current
  // trust interval, streak, and actions remaining until the next check-in.
  const updateStatus = (ctx: any) => {
    if (!ctx?.hasUI) return;
    const theme = ctx.ui.theme;
    const left = Math.max(0, trust.interval - sinceCheckin);
    ctx.ui.setStatus(
      "prosecutor",
      `${theme.fg("accent", `trust ${trust.interval}/16`)} ${theme.fg(
        "dim",
        `streak ${trust.streak} · check-in in ${left}`,
      )}`,
    );
  };

  pi.on("session_start", (_event: any, ctx: any) => updateStatus(ctx));

  pi.on("tool_execution_end", (event: any, ctx: any) => {
    if (event?.toolName !== "claim") sinceCheckin += 1;
    updateStatus(ctx);
  });

  pi.registerTool({
    name: "claim",
    description:
      "State ONE falsifiable claim about current progress. A skeptical prosecutor " +
      "with NO access to your reasoning verifies it against ground truth and either " +
      "clears you to continue or sends you back to recover.",
    parameters: claimSchema as never,
    async execute(_toolCallId: string, params: any, signal: any, _onUpdate: unknown, ctx: any) {
      const systemPrompt =
        "You are a skeptical prosecutor verifying a coding agent's claim. You do NOT trust it.\n" +
        `Goal context: ${goal || "(unspecified)"}\n` +
        "Run the suggested check and any cheap probes (tests, git diff, grep) to decide if the " +
        "claim is literally true right now. Trust nothing you cannot observe.\n" +
        'End with exactly one line: "VERDICT: UPHELD" or "VERDICT: BROKEN <one-line evidence>".';
      const prompt = `Claim: ${params.statement}\nSuggested check: ${params.verify}`;

      // The isolated verification can take up to two minutes; without a
      // working message the TUI just looks frozen.
      if (ctx?.hasUI) ctx.ui.setWorkingMessage("prosecutor verifying claim...");

      let verdict: { upheld: boolean; evidence: string };
      try {
        const res = await runIsolatedPi({
          prompt,
          systemPrompt,
          // The prosecutor reuses the executor's model (opus-4-8 / gpt-5.5
          // medium) by default; the asymmetry that matters is context isolation,
          // not a weaker model. Override per-run with PI_PROSECUTOR_* if desired.
          provider: process.env.PI_PROSECUTOR_PROVIDER ?? process.env.PI_PROVIDER,
          model: process.env.PI_PROSECUTOR_MODEL ?? process.env.PI_MODEL,
          thinking: process.env.PI_PROSECUTOR_THINKING ?? process.env.PI_THINKING,
          cwd: ctx?.cwd,
          timeoutMs: 120000,
          signal,
        });
        verdict = parseVerdict(`${res.stdout}\n${res.stderr}`);
      } catch (err) {
        verdict = { upheld: false, evidence: `prosecutor failed to run: ${String(err)}` };
      } finally {
        if (ctx?.hasUI) ctx.ui.setWorkingMessage(undefined);
      }

      const t = trust.record(verdict.upheld);
      sinceCheckin = 0;

      if (ctx?.hasUI) {
        if (verdict.upheld) {
          ctx.ui.notify(`Claim UPHELD - trust interval now ${t.interval}`, "info");
        } else {
          ctx.ui.notify(`Claim BROKEN: ${verdict.evidence}`, "error");
        }
        updateStatus(ctx);
      }

      // Persist the verdict out of the model's context for replay/audit.
      pi.appendEntry("prosecutor", {
        statement: params.statement,
        verify: params.verify,
        upheld: verdict.upheld,
        evidence: verdict.evidence,
        interval: t.interval,
        streak: t.streak,
      });

      const text = verdict.upheld
        ? `UPHELD. Trust interval widened to ${t.interval}. Continue.`
        : `BROKEN: ${verdict.evidence}. Do not proceed on this assumption; recover and re-establish it.`;
      return { content: [{ type: "text", text }], details: { isError: !verdict.upheld } };
    },
  });
}
