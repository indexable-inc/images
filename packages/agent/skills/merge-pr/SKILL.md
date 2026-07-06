---
name: merge-pr
description: Autonomously drive a pull request to merged by spawning a background subagent (the fork) that watches CI fail-fast, fixes each failure on the PR branch, re-pushes, and merges when green, all without blocking the main session. Use when the user wants to watch and merge a PR, babysit it, fix CI and merge, get a PR in, or otherwise hand off a PR's CI-to-merge loop so they can keep working. The skill never blocks the main thread; it launches the watcher and returns immediately.
---

# merge-pr

Hand a PR's whole "watch CI → fix failures → merge" loop to a **background subagent** so the main session stays live. The skill's job is to identify the PR, launch the fork, and return control immediately. The fork does the grind and reports back via a completion notification.

## What the skill does (main thread)

1. **Resolve the target PR.**
   - If the user named a PR number / URL, use it.
   - Otherwise resolve the PR for the current branch: `gh pr view --json number,headRefName,headRepository,baseRefName,url,state`.
   - If there is no PR for the branch and the branch has unpushed commits, that is a fork in the road: ask whether to open one first (or open it if the user already said "push and merge"). Do not invent a PR.

2. **Spawn the fork** with the `Agent` tool, `run_in_background: true`, `model: opus`. Pass it the PR number, repo, and the charter below verbatim (filled in). Then **tell the user one line**: which PR is being watched and that the loop is running in the background. Do not foreground-wait.

3. **On the completion notification**, relay the outcome: merged (with commit), or stopped-stuck (with the failure it could not fix and why), or blocked (needs a human decision).

Keep all CI watching inside the fork, and even there make it event-driven (see below). A foreground `gh pr checks --watch` / `gh run watch` in the main thread holds a Bash slot up to the 600s timeout and freezes the session, so route it through `run_in_background` + `Monitor` instead.

## The fork's charter (paste into the subagent prompt)

> You own PR #<N> in <owner/repo> from now until it is merged or you are genuinely stuck. Loop:
>
> 1. **Watch CI fail-fast, silently.** Arm exactly ONE event-driven watch and let it be the only thing that wakes you. It must complete only on a terminal or changed state — a check result (any fail / all pass), a new head sha, a merge or close event, or your own deadline — never on a timer tick that re-reports "pending". Run `gh pr checks <N> --watch --fail-fast --interval 20` with `run_in_background: true` (never a foreground Bash slot) so it returns the instant ANY check fails or all pass. `--fail-fast` needs gh ≥ 2.42; if it is rejected, drop the flag and run a background `until` loop over `gh pr checks <N> --json name,state,bucket` that exits only when the aggregate state differs from when you armed it.
> 2. **If all checks pass → merge.** Use the repo's merge norm: for `indexable-inc/index` and `indexable-inc/ix`, admin force-merge immediately (`gh pr merge <N> --admin --squash`; merge queue is on, so do NOT pass `--delete-branch`). For other repos, enable auto-merge (`gh pr merge <N> --auto --squash`) unless the user said force-merge. Then report "merged" with the squash commit sha and stop.
> 3. **If a check failed → diagnose and fix.**
>    - Pull the failing logs: `gh pr checks <N>` to find the failed check, then `gh run view <run-id> --log-failed` (or `--log` for the failing job) to read the actual error. Read the real error, do not guess.
>    - Work on the PR branch in an **isolated worktree** off the PR head, never the shared main checkout (in index/ix a PreToolUse hook blocks edits on main anyway). `git fetch origin` then `git worktree add ../<repo>-pr<N> <headRef>` and fix there. Validate the fix at the layer it touches via the repo's own commands (e.g. `nix build .#…`, `nix run .#lint`, the scoped `nix build .#checks.<system>.<name>`) before pushing, so you are not burning CI to test a guess.
>    - Commit (match the repo's commit style) and `git push`. Remove the worktree when done.
> 4. **Re-watch from step 1.**
>
> **Stuck detection (do not loop forever).** Keep a short signature of each failure (check name + the load-bearing error line). If the SAME failure recurs after your fix, or you see 3 consecutive failed CI rounds without net progress, STOP and report: the failing check, the error, what you tried, and why you could not fix it. A flaky/transient infra failure (timeout, runner died, cache outage) is not your bug: re-run it once (`gh run rerun <run-id> --failed`) and only escalate if it fails again the same way.
>
> **Escalate, do not guess, when:** the fix needs a product decision, the failure is in code you did not touch and the right fix is ambiguous, a required check needs human approval, or merging is blocked by a review the user must give. Report the specific blocker.
>
> **Watch the clock.** If a single CI round runs far longer than that repo's checks normally take, investigate the long-pole step rather than waiting it out (in index/ix, treat any check over ~1 min as a problem to look at, not wait on).
>
> **Report only on state change.** Every turn-end message you write lands as a task notification in the parent session, so a "still pending, waiting" update is pure noise. Never emit a no-change status report; if you wake and nothing changed, re-arm and yield silently. Batch progress into state-change reports only (a check failed, you pushed a fix, merged, stuck, blocked). Yielding silently never means stopping: keep a live watch armed whenever the outcome is pending, and re-arm it BEFORE ending the turn.
>
> Report concisely when done: final state (merged + sha / stopped-stuck / blocked), and for anything you fixed, one line per fix.

## Notes

- The fork is a real background `Agent`, so the user keeps working while it runs; its final message arrives as a task-completion notification. Relay what matters, do not make the user read the transcript.
- If the user wants to watch several PRs, spawn one fork per PR in a single message so they run concurrently.
- This composes with, rather than replaces, repo-local skills: inside `indexable-inc/index` the `ci-merge` skill already encodes that repo's watch-fix-merge specifics, so the fork should prefer it there.
- Keep any commit/PR text the fork writes short and human, and add the one-line AI attribution on anything outward-facing, per global style.
