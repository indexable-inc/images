---
name: review-changes
description: "Run a multi-agent adversarial review over the current working-tree changes. Fans out one finder per dimension (correctness, security, performance, maintainability) over the git diff, adversarially verifies each finding to cut false positives, and returns ranked blockers vs warnings. Invoke after completing a substantive change, or when the Stop review gate asks for it. This reviews a finished local diff, not a remote PR (use /review for a PR)."
---

# review-changes

Launch a multi-agent review of the **working-tree diff** through the Workflow
tool, then act on the confirmed blockers. This is the worker behind the
always-on Stop review gate (`review-gate.py`); it is also fine to invoke
directly.

## When to run

- The Stop gate blocked with "run the review-changes skill", or
- you just finished a substantive change and want the adversarial pass before
  declaring it done.

Skip for genuinely trivial changes (a typo, a one-line doc/comment, a config
value with no logic). Say so in one line and stop.

## How to run

1. Find the change context: `cwd` = repo root via `git rev-parse --show-toplevel`
   (fall back to the current directory if not in a git tree). The review targets
   `git diff` plus `git diff --staged`; if both are empty because the change was
   already committed, the finders fall back to `git show HEAD`.

2. Call the **Workflow** tool with the committed review script (do not inline a
   script of your own):

   ```
   Workflow({
     scriptPath: "/Users/andrewgazelka/.config/nix/claude/global/skills/review-changes/review.workflow.js",
     args: { cwd: "<repo root>" }
   })
   ```

   The workflow runs in the background and notifies on completion. It fans out
   one `code-reviewer` finder per dimension over the diff, pipelines each finding
   through an independent skeptic that tries to refute it, and returns the
   surviving findings ranked, with Correctness and Security marked as blockers.

3. When it completes, report the verdict to the user, blockers first. Fix the
   confirmed Correctness/Security findings in the same turn, or state plainly why
   a finding is declined. Performance and Maintainability are the author's call.
   If the workflow returns zero confirmed findings, say so in one line and move
   on.

## Notes

- The review is read-only; it never edits. You apply the fixes.
- The workflow reuses the global `code-reviewer` agent as its finder, so findings
  follow the same severity and evidence discipline as a manual reviewer pass.
