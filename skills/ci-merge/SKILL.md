---
description: Watch CI, fix failures, and enable auto-merge when passing
---

Push the current branch and watch CI. If CI fails, fix the issues and retry until it passes. Once CI passes, enable auto-merge.

## Instructions

1. **Run local checks first** (if applicable):
   - Look for `ci.sh`, `Makefile`, or similar in the repo
   - For Rust: run the nearest Nix package or check owner for the touched surface
   - For TypeScript/JS: `bun run lint && bun run typecheck && bun test`
   - Fix any issues before pushing - faster feedback than waiting for remote CI

2. **Check for PR**: Run `gh pr view --json number -q .number`
   - If no PR exists for current branch, create one as **non-draft** (required for auto-merge): `gh pr create --fill` (do NOT use `--draft`) and open in browser with `gh pr view --web`

3. **Push current branch**: `git push` (or `git push -u origin HEAD` if no upstream)

4. **Watch CI**: Use the `pr-check-watch` agent to monitor CI status (lightweight, uses less context)

5. **On failure**:
   - Read the failing check logs with `gh run view <run-id> --log-failed`
   - Analyze the error and fix it
   - Commit the fix (amend if it's a small fix to the same logical change, otherwise new commit)
   - Force push if amended, regular push otherwise
   - Go back to step 4

6. **Stop conditions**:
   - CI passes - proceed to step 7
   - Error requires major architectural changes - stop and explain to user
   - Error is external/flaky (network, service outage) - retry once, then stop and explain
   - Stuck in loop (same error 3+ times) - stop and ask user for help

7. **Enable auto-merge**: Once CI passes, run `gh pr merge --auto --squash` to enable auto-merge with squash

8. **Post-merge cleanup**: After the PR is merged:
   - Get the default branch: `gh repo view --json defaultBranchRef -q .defaultBranchRef.name`
   - Switch to it: `git switch <default-branch>`
   - Delete the merged branch locally: `git branch -d <merged-branch-name>`

9. **Report**: Summarize what was fixed (if anything), final CI status, and confirm auto-merge is enabled
