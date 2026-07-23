import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { rank, scoreBranch } from "./scoring.js";
import { countDiffLines, runProcess as sh } from "./process.js";

// At runtime this file lives at share/<name>/lib/fanout.js, so the turn-cap
// extension (an aux file) sits one directory up.
const here = dirname(fileURLToPath(import.meta.url));
const turnCapExt = join(here, "..", "turn-cap.js");

// Run each approach on its own detached worktree off HEAD, under a soft turn
// cap (turn-cap extension) and a hard wall-clock cap (`timeout`), then score by
// ground truth. Branches start from the last commit, not the dirty working
// tree - explore from a clean base.
export async function fanout({
  approaches,
  goal,
  repoRoot,
  scoreCmd,
  provider,
  model,
  thinking,
  turnCap = 6,
  timeoutSec = 180,
}) {
  const branches = await Promise.all(
    approaches.map(async (approach, i) => {
      const wt = await mkdtemp(join(tmpdir(), `beam-${i}-`));
      const added = await sh("git", ["-C", repoRoot, "worktree", "add", "-q", "--detach", wt, "HEAD"]);
      if (added.code !== 0) {
        return {
          approach,
          exitCode: 1,
          diffLines: 0,
          patch: "",
          scoreOut: `worktree add failed: ${added.stderr}`,
          score: scoreBranch({ exitCode: 1, diffLines: 0 }),
        };
      }

      try {
        const piArgs = ["--print", "--no-session", "-e", turnCapExt];
        if (provider) piArgs.push("--provider", provider);
        if (model) piArgs.push("--model", model);
        if (thinking) piArgs.push("--thinking", thinking);
        piArgs.push(`Goal: ${goal}\n\nPursue ONLY this approach and implement it: ${approach}`);

        await sh("timeout", [`${timeoutSec}s`, "pi", ...piArgs], {
          cwd: wt,
          env: { ...process.env, PI_TURN_CAP: String(turnCap) },
        });

        const diff = await sh("git", ["-C", wt, "diff", "--no-color"]);
        const numstat = await sh("git", ["-C", wt, "diff", "--numstat"]);
        const diffLines = countDiffLines(numstat.stdout);
        const scored = scoreCmd ? await sh("bash", ["-lc", scoreCmd], { cwd: wt }) : { code: 0, stdout: "" };

        return {
          approach,
          exitCode: scored.code,
          diffLines,
          patch: diff.stdout,
          scoreOut: (scored.stdout || "").slice(-2000),
          score: scoreBranch({ exitCode: scored.code, diffLines }),
        };
      } finally {
        await sh("git", ["-C", repoRoot, "worktree", "remove", "--force", wt]);
        await rm(wt, { recursive: true, force: true }).catch(() => {});
      }
    }),
  );

  return rank(branches);
}
