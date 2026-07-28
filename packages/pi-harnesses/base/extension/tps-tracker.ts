import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";

// Live tokens-per-second in the footer while the model streams, plus a final
// per-run summary toast. Until the provider reports official usage, output is
// estimated at ~4 chars/token from streamed deltas.
//
// Adapted from davis7dotsh/my-pi-setup (MIT, (c) 2026 Benjamin Davis).
export default function (pi: ExtensionAPI) {
  let messageStart: number | null = null;
  let streamStart: number | null = null;
  let estimatedStreamedTokens = 0;
  let totalOutputTokens = 0;
  let totalStreamMs = 0;

  pi.on("agent_start", async (_event: any, ctx: any) => {
    totalOutputTokens = 0;
    totalStreamMs = 0;
    messageStart = null;
    streamStart = null;
    estimatedStreamedTokens = 0;
    if (!ctx.hasUI) return;
    ctx.ui.setStatus("tps", ctx.ui.theme.fg("dim", "generating..."));
  });

  pi.on("message_start", async (event: any) => {
    if (event.message.role !== "assistant") return;
    messageStart = Date.now();
    streamStart = null;
    estimatedStreamedTokens = 0;
  });

  pi.on("message_update", async (event: any, ctx: any) => {
    if (event.message.role !== "assistant") return;

    const streamEvent = event.assistantMessageEvent;
    const isOutputDelta =
      streamEvent.type === "text_delta" ||
      streamEvent.type === "thinking_delta" ||
      streamEvent.type === "toolcall_delta";
    if (!isOutputDelta) return;

    const now = Date.now();
    streamStart ??= now;
    estimatedStreamedTokens += Math.max(0, (streamEvent.delta?.length ?? 0) / 4);

    if (!ctx.hasUI) return;
    const elapsed = (now - streamStart) / 1000;
    const officialTokens = event.message.usage?.output ?? 0;
    const currentTokens = officialTokens > 0 ? officialTokens : estimatedStreamedTokens;

    if (elapsed > 0 && currentTokens > 0) {
      const tps = Math.round(currentTokens / elapsed);
      const tokenLabel =
        officialTokens > 0 ? `${officialTokens} tok` : `~${Math.round(estimatedStreamedTokens)} tok`;
      const theme = ctx.ui.theme;
      ctx.ui.setStatus(
        "tps",
        `${theme.fg("accent", `${tps} tok/s`)} ${theme.fg("dim", `(${tokenLabel} / ${elapsed.toFixed(1)}s)`)}`,
      );
    }
  });

  pi.on("message_end", async (event: any) => {
    if (event.message.role !== "assistant") return;

    const messageTokens = event.message.usage?.output ?? 0;
    const timingStart = streamStart ?? messageStart;
    if (!timingStart || messageTokens <= 0) {
      messageStart = null;
      streamStart = null;
      estimatedStreamedTokens = 0;
      return;
    }

    totalOutputTokens += messageTokens;
    totalStreamMs += Math.max(0, Date.now() - timingStart);

    messageStart = null;
    streamStart = null;
    estimatedStreamedTokens = 0;
  });

  pi.on("agent_end", async (_event: any, ctx: any) => {
    if (!ctx.hasUI) return;
    const elapsed = totalStreamMs / 1000;
    const tps = totalOutputTokens > 0 && elapsed > 0 ? Math.round(totalOutputTokens / elapsed) : 0;

    const theme = ctx.ui.theme;
    const tpsLabel = tps > 0 ? theme.fg("accent", `${tps} tok/s`) : theme.fg("dim", "N/A");
    const detail = theme.fg("dim", `${totalOutputTokens} tokens in ${elapsed.toFixed(1)}s streaming`);

    ctx.ui.notify(`${tpsLabel}  ${detail}`, "info");
    ctx.ui.setStatus("tps", theme.fg("dim", `done - ${tpsLabel}`));
  });
}
