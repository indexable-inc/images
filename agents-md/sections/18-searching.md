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

Search before claiming external facts, API behavior, flags, versions, or current
ownership. Live state beats docs when the task is about a running system; if
observers disagree, debug the observer path too.

Debug from first principles: actor, operation, boundary, invariant, observer.
Prove the broken boundary with the smallest live check, then fix the owner.
