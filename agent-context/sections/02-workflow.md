---
name: workflow
disclosure: progressive
description: "Branch, worktree, and PR workflow: starting work, opening a PR to main, watching checks, handling review threads, merging. Use when committing, pushing, or managing a PR."
---

## Workflow

Pull `main` before starting. Always make changes on a short-lived branch in a
separate worktree by default, including small docs edits. Keep the shared `main`
checkout as the clean landing zone for pulls, branch bases, and final syncs.

Create the branch and worktree from the updated `main` checkout. Use the
`codex/` branch prefix unless the user asks for a different name:

```sh
git worktree add ../<short-name>-<branch> -b codex/<branch> main
```

If the shared checkout already has unrelated edits, name the paths and the one
line summary of what they appear to be doing before creating the new worktree.
Avoid stashing operator work out of the way.

After local checks pass, push the branch and open a PR targeting `main`. Enable
auto-merge as soon as required checks and review state allow it. Watch required
checks with `gh pr checks --watch --fail-fast`; if a check fails, inspect the
run logs, fix the branch, push again, and restart the watcher. Keep that loop
going until GitHub reports the PR merged or a human explicitly asks you to stop.

`gh pr checks` may show stale failed runs next to newer passing reruns for the
same check name. When the output is mixed, inspect
`gh pr view --json mergeStateStatus,statusCheckRollup,latestReviews` and trust
the latest run for the current head SHA rather than the oldest failure in the
list.

Treat PR comments and reviews as part of the work. Read them with
`gh pr view --comments` and the review fields from `gh pr view --json reviews`.
Address AI review comments in code when they identify a real issue, reply when
a comment is intentionally declined, and resolve review threads before relying
on auto-merge. The AI review gate is the default code review signal for
agent-authored PRs; do not add or preserve a separate GitHub code-quality lane
unless the user asks for it.

Check the PR author before pushing to, closing, merging, enabling auto-merge for,
or otherwise modifying a PR. Do not change PRs authored by another GitHub user
unless that user or the operator explicitly authorizes it.

AI review inline feedback lives in GitHub review threads, which `gh pr view
--comments` does not show. Inspect unresolved threads directly before deciding a
PR is clear:

```sh
gh api graphql --paginate \
  -f owner=<owner> -f repo=<repo> -F number=<pr> \
  -f query='query($owner:String!,$repo:String!,$number:Int!,$endCursor:String){ repository(owner:$owner,name:$repo){ pullRequest(number:$number){ reviewThreads(first:100,after:$endCursor){ pageInfo{ hasNextPage endCursor } nodes{ id isResolved path line comments(first:100){ pageInfo{ hasNextPage endCursor } nodes{ author{login} body url } } } } } } }'
```

If a thread reports `comments.pageInfo.hasNextPage`, page that thread's comments
before declaring it resolved:

```sh
gh api graphql --paginate \
  -f thread=<thread-id> \
  -f query='query($thread:ID!,$endCursor:String){ node(id:$thread){ ... on PullRequestReviewThread{ comments(first:100,after:$endCursor){ pageInfo{ hasNextPage endCursor } nodes{ author{login} body url } } } } }'
```

Unresolved AI review threads are immediate blockers. Do not wait on more checks
when the reviewer has left an open thread: fix the code or resolve the thread
with the GitHub review-thread API. If GitHub does not rerun the failed gate for
the current head, rerun it with `gh run rerun <run-id> --failed`.

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

`main` is the long-lived human branch. PRs target `main`. Deployment refs are
tags on commits that are already reachable from `main`.

Contributor setup and local checks live in [`CONTRIBUTING.md`](CONTRIBUTING.md).
Run the repo lint before committing:

```sh
nix run .#lint
```
