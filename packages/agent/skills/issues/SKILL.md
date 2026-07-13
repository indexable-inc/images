---
name: issues
description: "Filing and formatting GitHub issues: short bodies, repro steps, labels, multiline input. Use when creating or editing an issue."
---

## Issues

Keep issue bodies short: problem, context, desired outcome. Bug reports need a
concrete reproduction command or step list. Avoid prescribing implementation
unless that is the actual request.

When creating or editing GitHub issue bodies or comments, pass multiline text
through a real multiline input path such as `--body-file -`, a temporary file, or
an editor. Escaped `\n` sequences in quoted `--body` strings render literally on
GitHub.

Prefer GitHub's suggestion block syntax for proposed inline changes in PR review
comments on changed lines. Use fenced `suggestion` blocks only when GitHub can
apply the snippet directly.

When work exposes a real bug, broken assumption, or unidiomatic pattern that
will outlive the current task, file a GitHub issue right then. One concrete
observation per issue.

Before filing a code-level issue derived from a running or profiled binary (a
stack trace, symbol, or hot function), pin the binary to a repo+rev: the build
path often embeds the rev, `git cat-file -t <rev>` in each candidate repo
confirms the owner, and `gh api search/code` on a quoted symbol finds the
source. Then confirm the symbol still exists on that repo's main; if it is
gone, the fix already landed, so cite the removing PR instead of filing.

Apply labels at filing time. Use labels to make the next action sortable:
`bug`, `enhancement`, `documentation`, `rfc`, `help wanted`, `good first issue`,
and `ai-capable` when an agent can plausibly finish the issue from the body
alone.
