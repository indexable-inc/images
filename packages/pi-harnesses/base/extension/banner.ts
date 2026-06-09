import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";

// One-time toast on session start so it is self-evident the pack is active and
// what it added. Everything else in pi-base is intentionally quiet at rest
// (widget only in git repos, tok/s only while streaming, commands on demand).
export default function (pi: ExtensionAPI) {
  pi.on("session_start", async (event: any, ctx: any) => {
    if (!ctx.hasUI) return;
    if (event?.reason !== "startup" && event?.reason !== "new") return;
    ctx.ui.notify(
      "pi-base UX active: live tok/s (footer, while streaming) · git widget (above editor, in repos) · /diff · /lg",
      "info",
    );
  });
}
