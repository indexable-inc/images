// Deterministic per-run turn cap, as a standalone Pi extension.
//
// Pi exposes no --max-turns flag, so beam branches load this extension with
// `-e turn-cap.js` and the runner sets PI_TURN_CAP. It counts model response
// cycles (turn_end) and aborts the run once the cap is hit. This is the soft,
// graceful cap; the runner also wraps each branch in a wall-clock `timeout` as
// the hard guarantee.
export default function (pi) {
  const cap = Number.parseInt(process.env.PI_TURN_CAP ?? "6", 10);
  let turns = 0;
  pi.on("turn_end", (_event, ctx) => {
    turns += 1;
    if (turns >= cap && ctx && typeof ctx.abort === "function") {
      ctx.abort();
    }
  });
}
