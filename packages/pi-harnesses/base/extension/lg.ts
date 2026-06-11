import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";

// /lg - ask the agent for a concise summary of unstaged changes with per-file
// +/- counts. If the agent is mid-turn, queue it as a follow-up.
//
// Adapted from davis7dotsh/my-pi-setup (MIT, (c) 2026 Benjamin Davis).
const LG_PROMPT = `Run git status, inspect what has changed, then respond with only:

1. A short 1-2 sentence summary of the unstaged changes.
2. A list of changed unstaged files with their +/- line counts.
3. A total +/- line count at the bottom.

Keep it concise. Use git commands to calculate the line counts; do not include staged changes unless they also have unstaged modifications.`;

export default function (pi: ExtensionAPI) {
  pi.registerCommand("lg", {
    description: "Summarize unstaged git changes with per-file +/- counts",
    handler: async (_args: string, ctx: any) => {
      if (!ctx.isIdle()) {
        pi.sendUserMessage(LG_PROMPT, { deliverAs: "followUp" });
        ctx.ui.notify("Queued /lg after the current turn finishes.", "info");
        return;
      }
      pi.sendUserMessage(LG_PROMPT);
    },
  });
}
