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
Address Codex comments in code when they identify a real issue, reply when a
comment is intentionally declined, and resolve review threads before relying on
auto-merge. Codex is the default code review signal for agent-authored PRs; do
not add or preserve a separate GitHub code-quality lane unless the user asks for
it.

Codex inline feedback lives in GitHub review threads, which `gh pr view
--comments` does not show. Inspect unresolved threads directly before deciding a
PR is clear:

```sh
gh api graphql \
  -f owner=<owner> -f repo=<repo> -F number=<pr> \
  -f query='query($owner:String!,$repo:String!,$number:Int!){ repository(owner:$owner,name:$repo){ pullRequest(number:$number){ reviewThreads(first:100){ nodes{ id isResolved path line comments(first:50){ nodes{ author{login} body url } } } } } } }'
```

Unresolved Codex review threads are immediate blockers. Do not wait on more
checks when Codex has left an open thread: fix the code or resolve the thread
with the GitHub review-thread API, then request a fresh Codex review if the head
changed. If the head did not change and GitHub does not rerun the failed gate,
rerun it with `gh run rerun <run-id> --failed`.

When manually triggering Codex, include the full current head SHA in the request,
for example `@codex review head <sha>`. This gives no-findings responses a
specific revision to answer. Avoid sending a later generic `@codex review` for
the same head because it weakens that audit trail.

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

