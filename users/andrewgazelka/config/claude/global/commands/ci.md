# CI Watch and Fix Loop

Push the current branch and watch CI. If CI fails, fix the issues and retry until it passes.

## Instructions

1. **Run local checks first** (if applicable):
   - Look for `ci.sh`, `Makefile`, or similar in the repo
   - For Rust: `cargo fmt --check && cargo clippy -- -D warnings && cargo test`
   - For TypeScript/JS: `bun run lint && bun run typecheck && bun test`
   - Fix any issues before pushing - faster feedback than waiting for remote CI

2. **Check for PR**: Run `gh pr view --json number -q .number`
   - If no PR exists for current branch, create one: `gh pr create --draft --fill` and open in browser with `gh pr view --web`

3. **Push current branch**: `git push` (or `git push -u origin HEAD` if no upstream)

4. **Watch CI**: Use the `pr-check-watch` agent to monitor CI status (lightweight, uses less context)

5. **On failure**:
   - Read the failing check logs with `gh run view <run-id> --log-failed`
   - Analyze the error and fix it
   - Commit the fix (amend if it's a small fix to the same logical change, otherwise new commit)
   - Force push if amended, regular push otherwise
   - Go back to step 4

6. **Stop conditions**:
   - CI passes - done, report success
   - Error requires major architectural changes - stop and explain to user
   - Error is external/flaky (network, service outage) - retry once, then stop and explain
   - Stuck in loop (same error 3+ times) - stop and ask user for help

7. **Report**: When done, summarize what was fixed (if anything) and final CI status
