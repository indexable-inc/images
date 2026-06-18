---
name: searching
description: "Searching this codebase, external repos, and fleet history: semantic (mgrep, search.semantic) vs exact (rg/fd), cloning upstreams to /tmp. Use when locating code, researching an API or pattern, or checking what prior agents already did."
---

## Searching

Use exact text search for exact questions and semantic search for fuzzy
questions. Prefer machine-readable output when available, then inspect the narrow
source files that own the behavior.

Reach for semantic search first on conceptual or natural-language questions:
`mgrep search -c "<query>" <path>` returns ranked files with the matched
snippets. Add `-r` to recurse into subdirectories, `-m N` to raise the
ten-result cap, `-a` to synthesize an answer from the hits, `-w` to fold in web
results, and `--agentic` to let mgrep refine the query across several searches.
Pass `-s` to sync local edits into the store before searching when files changed
since the last index, or run `mgrep watch` to keep the store live. Use `rg` for
exact strings and known symbols, and `fd` or a glob tool for known file-path
patterns.

Avoid broad agent delegation for simple search. The codebase is usually small
enough that direct search plus a focused read gives better signal.

When reading source from another repository, clone it once into `/tmp` and
search the clone with `rg` and `fd` instead of curling individual files. A
local clone lets one query find every call site, follows renames, and avoids
guessing which file holds the answer. Use `git clone --depth=1
https://github.com/<owner>/<repo> /tmp/<repo>` for a fast read-only checkout
and delete the directory when the question is answered.

Before settling on an API shape, helper structure, or library pattern, spawn a
subagent to check what the current idiom is. Pair `mcp__exa__web_search_exa`
with `/tmp` clones of the maintained upstreams so the subagent can read real
call sites and recent release notes, then report back a short summary. Cheap
research up front beats shipping a pattern that the ecosystem has already moved
past.

## Fleet priors

The shared store also indexes the whole fleet's history, not just code. From
the index kernel: `import search`, then `await search.semantic("<task
phrasing>", source=["claude_history"], top_k=5)` — each verb is async and
returns a polars frame (one row per hit; `.filter`/`.group_by`/`.head` it).
Route by question type:
`shell` answers "what is the command" for a few hundred tokens, `github`
answers "why is it this way" with PR bodies and URLs, `claude_history` answers
"how did someone do this". Pass `agentic=True` only for failure-shaped queries
(slower, but it rescues them). The corpus knows prior decisions, known
pitfalls, and whether the thing is already built; finding a prior "this is
~90% built on main" beats reimplementing it.

The canonical source tags are `claude_history`, `codex`, `shell`,
`claude_debug`, `github`, and `code`. Nothing else exists: a wrong tag
(`atuin`, `slack`, `linear`, `git_log`) returns 0 hits silently instead of
erroring, so treat an empty result as a possible typo before concluding the
corpus is empty. Narrow with `user=["<name>"]`, `host=[...]`,
`project=[...]`, or `repo=` when you know whose history holds the answer.

Skip the prior search for trivial, local-only edits (a rename, a typo, a
small refactor): generic phrasings retrieve noise and the round trip costs
more than it saves. Reach for it when the task touches infra, deploy, CI,
conventions, debugging, or anything another agent has plausibly done before,
and prefer a cheap-model subagent for broad prior research so raw hits never
flood the main context.

Search before claiming external facts, API behavior, flags, versions, or current
ownership. Live state beats docs when the task is about a running system; if
observers disagree, debug the observer path too.

Debug from first principles: actor, operation, boundary, invariant, observer.
Prove the broken boundary with the smallest live check, then fix the owner.
