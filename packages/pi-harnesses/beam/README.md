# pi-beam

An executor that turns a hard decision into a bounded beam search instead of a
linear commitment.

## How it works

1. At a decision point the executor calls
   `explore({ approaches: [...], score: "cargo check" })`.
2. Each approach runs on its own detached git worktree (off `HEAD`) as an
   isolated `pi --print` subprocess, under two budgets:
   - a soft turn cap via the `turn-cap` extension (`PI_TURN_CAP`), since Pi has
     no `--max-turns` flag, and
   - a hard wall-clock cap via `timeout`.
3. Branches are scored on GROUND TRUTH by code (`shared/ext-lib/scoring.js`): the
   `score` command's exit code dominates, then smaller diff wins.
4. The ranked results plus the winning patch return to the executor, which
   applies the winning patch itself. Beam proposes; the executor commits.

Dead ends die in a few turns instead of after the executor commits to one bad
path for forty.

## Pieces

- `extension/beam.ts` - the `explore` tool.
- `runner/fanout.js` - worktree fan-out, budgeting, diff capture.
- `shared/ext-lib/scoring.js` - pure ranking (unit-tested).
- `shared/ext-lib/turn-cap.js` - per-branch soft turn budget.

## Notes & limits

- Branches start from the last commit, not the dirty working tree. Explore from
  a clean base, or commit/stash first.
- Adoption is executor-driven in this first cut (it applies the returned patch).
  Auto-adoption via `git apply`/session graft is a follow-up.
- The in-process SDK fan-out (`createAgentSessionRuntime`) is the Tier-2 upgrade
  to the subprocess runner here.

## Run

```
ANTHROPIC_API_KEY=... nix run .#pi-beam -- "refactor the auth module; explore 3 designs"
```
