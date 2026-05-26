## Searching

Use exact text search for exact questions and semantic search for fuzzy
questions. Prefer machine-readable output when available, then inspect the narrow
source files that own the behavior.

Avoid broad agent delegation for simple search. The codebase is usually small
enough that direct search plus a focused read gives better signal.

Search before claiming external facts, API behavior, flags, versions, or current
ownership. Live state beats docs when the task is about a running system; if
observers disagree, debug the observer path too.

Debug from first principles: actor, operation, boundary, invariant, observer.
Prove the broken boundary with the smallest live check, then fix the owner.
