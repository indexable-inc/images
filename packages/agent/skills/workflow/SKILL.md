---
name: workflow
description: "Branch, worktree, and PR workflow: starting work, opening a PR to main, watching checks, handling review threads, merging. Use when committing, pushing, or managing a PR."
---

## Workflow

Pull `main` before starting. Always make changes in a separate worktree by
default, including small docs edits. Keep the shared `main` checkout as the clean
landing zone for pulls, branch bases, and final syncs.

The default path to land a change is: verify locally, then push directly to
`main`. Run the repo gates on your change first (the lint -- `nix run .#lint` here,
`just lint` in ix, see the `linting` skill -- plus
`cargo check` / `cargo nextest run` on the affected packages, or a targeted `nix
build .#<pkg>` when the change touches a packaged artifact), then push. Do not
wait on CI to land a change. CI is advisory: a single shared runner node serves
the whole team, so routing routine validation through it overloads the node and
slows everyone down. Local verification is the gate that decides whether a change
is safe to push.

Push with a rebase loop, never force-push: if the push is rejected because `main`
moved, `git pull --rebase origin main` and push again.

Open a PR only when you want human review on a change, not as the default path
to land. When you do, create the branch and worktree from the updated `main`
checkout. Use the `codex/` branch prefix unless the user asks for a different
name. Place the worktree under `/tmp/<username>/` so it stays outside the flake
source tree and does not slow down Nix source-copy or lint walks:

```sh
git worktree add /tmp/<username>/<branch> -b codex/<branch> main
```

Never place worktrees under the repo root (e.g. `.claude/worktrees/` or
`.worktrees/`). A nested checkout adds tens of thousands of files to the flake
source set, which makes every `nix run .#...` re-ingest slow.

If the shared checkout already has unrelated edits, name the paths and the one
line summary of what they appear to be doing before creating the new worktree.
Avoid stashing operator work out of the way.

When you open a PR (the optional review path, not the default): after local
checks pass, push the branch and open a PR targeting `main`. CI checks on the PR
are signal, not a gate you are required to babysit, since local verification is
what decides a change is safe. Watch required checks with `gh pr checks --watch
--fail-fast` when you care about the result; if a check fails, inspect the run
logs, fix the branch, and push again. Do not block landing on a shared-runner CI
queue once local gates pass.

`gh pr checks` may show stale failed runs next to newer passing reruns for the
same check name. When the output is mixed, inspect
`gh pr view --json mergeStateStatus,statusCheckRollup,latestReviews` and trust
the latest run for the current head SHA rather than the oldest failure in the
list.

A check that is ABSENT is not a check that failed, and it is not a check that
passed. "Has not been dispatched yet" and "will never be dispatched" produce the
identical observation, and the only thing that separates them is waiting longer.
So an absence is not evidence until you know how long that check takes when it
works: a required context missing from a docs-only PR looks exactly like a path
filter that will never fire, and has twice turned out to be a five-minute runner
queue. Before reporting a check as structurally unreachable, find a merged PR of
the same shape and read how long it waited.

The same rule covers the opposite direction: do not read a required check as
passing because you did not find a failure. Require the names you expect to be
present AND successful, since a check that was never dispatched satisfies any
test written as the absence of red.

Treat PR comments and reviews as part of the work. Read them with
`gh pr view --comments` and the review fields from `gh pr view --json reviews`.

Check the PR author before pushing to, closing, merging, enabling auto-merge for,
or otherwise modifying a PR. Do not change PRs authored by another GitHub user
unless that user or the operator explicitly authorizes it.

Remove the worktree and delete the local branch after the PR has merged.

Commit one logical change at a time. Use the pathspec form so unrelated staged
or unstaged files cannot ride along:

```sh
git commit -m "scope: imperative subject" -- <paths>
```

Subjects are imperative, lowercased, and have no trailing period. The optional
scope names the layer being touched, such as `platform:`, `minecraft:`, or
`AGENTS:`. Use a body only for the reason the diff cannot show. If a commit
fixes a tracked GitHub issue, include `Fixes #123`, `Closes #123`, or
`Resolves #123` in the body. Use `Refs #123` for related or partial work.

The same verb choice is load-bearing for Linear, and getting it wrong strands
issues. In a PR description, `Fixes ENG-1234` or `Closes ENG-1234` creates a
closing link: Linear moves the issue to In Progress when the PR opens and to
Done when it merges. `Refs ENG-1234`, `Part of ENG-1234`, and a bare
`ENG-1234` mention create a non-closing link: the issue still jumps to In
Progress when the PR opens, but merging never moves it to Done, so it sits In
Progress forever. Pick the verb by outcome:

- The PR resolves the issue: write `Fixes ENG-1234` (or `Closes`) in the PR
  description. One closing verb per resolved issue; a mention in a commit body
  or a section heading does not count.
- The PR is partial or related work: write `Refs ENG-1234` and say in the PR
  body what remains.

Two traps follow from the automation. Opening any linked PR, even a `Refs`
one, drags an already-Done issue back to In Progress, and if that PR carries
no closing verb, nothing ever re-closes it; after merging a follow-up that
references a Done issue, check the issue and re-close it. And the merge
automation occasionally misses even a proper closing link, so after a `Fixes`
PR merges, verify the issue actually reached Done.

Every commit must reference the site page that explains it. Include a repo
path such as `packages/site/src/lib/updates/<slug>.svx` (pages under `plans/`
and `stories/` count too) anywhere in the message; the long-form description
belongs on that page, not in the commit body. The page must exist in the
commit's tree, so a page added in the same commit counts. Merge, fixup,
squash, and revert commits are exempt.

`main` is the long-lived human branch. PRs target `main`. Deployment refs are
tags on commits that are already reachable from `main`.

Contributor setup and local checks live in [`CONTRIBUTING.md`](CONTRIBUTING.md).
Run the repo lint before committing. In this repository that is:

```sh
nix run .#lint
```

It is `just lint` in `indexable-inc/ix`, which has no `nix run .#lint` at all.
The `linting` skill has the per-repo table.
