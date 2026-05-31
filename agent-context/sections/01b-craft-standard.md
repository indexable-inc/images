---
name: craft-standard
disclosure: always
description: "The craft bar for every change: clear ownership, typed boundaries, root-cause fixes, no silent fallbacks."
---

## Craft standard

Treat every change as work that someone else will read after the merge cools off.
Prefer the simplest design that makes the owner obvious. Compose small named
helpers and typed boundaries; avoid copy-pasted orchestration and anonymous
blobs of shape.

DRY up repeated mechanics when they express one domain fact. Do not hide
legitimately different behavior behind a forced abstraction; three similar
lines are better than a premature shared helper.

Implement core backend and domain logic once in Rust, then expose it to other
ecosystems through thin bindings rather than reimplementing it per language. A
Python, Wasm, or FFI surface should wrap a Rust crate that owns the behavior and
carry no domain logic of its own, so every caller shares one tested
implementation.

Refactor after the patch works. A passing diff with awkward ownership is not
done. Replace string-shaped domain values with typed constructors or
declarative helpers at the owner boundary instead of pushing parsing back into
callers.

Treat lint failures as design feedback. Suppress only for a deliberate local
invariant, a generated or external shape, or a documented tool limitation. Keep
the suppression small and explain it next to the line.

Treat repo rules as binding by default. Bypass one only when the local evidence
proves the rule is impossible or harmful for the task, and record that reason
near the exception.

Reject fallback paths. If an owner, route, capability, config, schema, or
transport is unavailable, return a typed error and make it observable rather
than guessing a safe default. Sentinel values, silent retries, and hidden
backstops trade today's reliability for tomorrow's debugging.

Tests defend behavior or invariants. Skip tests that mirror implementation
trivia; they pin the next refactor to today's shape without protecting any
caller.
