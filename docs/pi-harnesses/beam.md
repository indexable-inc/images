# pi-beam (bounded beam search)

`packages/pi-harnesses/beam` (flake output `pi-beam`) is an executor that turns a
hard decision into a bounded beam search instead of a linear commitment. At a
decision point the executor explores 2-4 candidate approaches in parallel on
isolated git worktrees under turn + time budgets, ranks them on ground truth, and
applies the winner. Dead ends die in a few turns instead of after the executor
commits to one bad path for forty (`beam/README.md`). See the collection
[overview](overview.md) for the shared builder and model table.

## The `explore` tool (`beam/extension/beam.ts`)

The extension registers one tool, `explore`, with this schema
(`beam/extension/beam.ts:13-42`):

- `approaches`: array of 2-4 distinct approach strings (`minItems: 2`,
  `maxItems: 4`).
- `score` (optional): a shell command whose exit code (0 = pass) and resulting
  diff size rank a branch, e.g. `cargo check` or `npm test`. Omit to rank on diff
  size only.
- `turnCap` (default 6): max model turns per branch (soft cap).
- `timeoutSec` (default 180): hard wall-clock cap per branch.

The goal is captured from `PI_BEAM_GOAL` or the first user message on
`agent_start` (`beam.ts:45-52`). On invocation `explore` resolves the repo root
(`git rev-parse --show-toplevel`, falling back to the ctx cwd) and calls
`fanout(...)` (`beam.ts:62-80`), passing the active model selection through
`PI_PROVIDER`/`PI_MODEL`/`PI_THINKING`. It returns a ranked table plus the winning
patch as text; the executor then applies the winning patch itself. Beam proposes,
the executor commits (`beam.ts:93-113`). It also records the run via
`pi.appendEntry("beam", {...})` for the transcript.

## Fan-out (`beam/runner/fanout.js`)

`fanout({approaches, goal, repoRoot, scoreCmd, provider, model, thinking,
turnCap, timeoutSec})` (`runner/fanout.js:47-106`) runs all approaches with
`Promise.all`. For each approach:

1. Create a temp dir and `git worktree add -q --detach <wt> HEAD` off the repo
   root (`fanout.js:60-71`). Branches start from the last commit, not the dirty
   working tree, so explore from a clean base or commit/stash first
   (`beam/README.md`). A failed `worktree add` yields a failing branch result.
2. Run `timeout <timeoutSec>s pi --print --no-session -e <turn-cap.js>
   [--provider/--model/--thinking] "<goal + approach>"` with the worktree as cwd
   and `PI_TURN_CAP` in env (`fanout.js:73-83`). Two budgets bound a branch: a
   soft turn cap (the `turn-cap` extension, since Pi has no `--max-turns` flag)
   and a hard wall-clock cap (`timeout`).
3. Capture `git diff --no-color` (the patch) and `git diff --numstat` to count
   changed lines (`countDiffLines`, `fanout.js:29-41`), then run the `score`
   command in the worktree (`fanout.js:85-88`).
4. Always remove the worktree (`git worktree remove --force`) and temp dir in a
   `finally` (`fanout.js:98-101`).

Each branch result is `{approach, exitCode, diffLines, patch, scoreOut, score}`,
and `fanout` returns `rank(branches)`.

## Scoring (`shared/ext-lib/scoring.js`)

Ground truth decides, not a model (`shared/ext-lib/scoring.js:1-13`):

```
scoreBranch({exitCode, diffLines}) = (exitCode === 0 ? 1 : 0) * 1e6 - (diffLines ?? 0)
```

A branch that passes its score command (exit 0) always beats one that fails;
among passers, the smaller diff wins (less churn for the same result).
`rank(branches)` sorts by descending score. The function is pure and unit-tested
(`beam/test/scoring.test.mjs`, run at build time via `checkFiles`).

## How it is built

`beam/default.nix` calls `../shared/mk-pi-harness.nix` with `lockdown = false`,
`session = true`, the `beam.ts` entry extension, `runner/fanout.js` +
`shared/ext-lib/scoring.js` as `libFiles`, and `shared/ext-lib/turn-cap.js` as an
`auxFile` (loaded by branch subprocesses via absolute path, deliberately NOT
auto-loaded into the main executor, which is not turn-capped)
(`beam/default.nix:13-37`). Branches need the full tool surface to actually
implement an approach, hence no lockdown.

```
ANTHROPIC_API_KEY=... nix run .#pi-beam -- "refactor the auth module; explore 3 designs"
```

## Limits

From `beam/README.md`: branches start from the last commit, not the working tree;
adoption is executor-driven in this first cut (it applies the returned patch;
auto-adoption via `git apply`/session graft is a follow-up); and the in-process
SDK fan-out (`createAgentSessionRuntime`) is the Tier-2 upgrade to this
subprocess runner.
