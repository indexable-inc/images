## Craft standard

Treat every change as work that someone else will read after the merge cools off.
Prefer the simplest design that makes the owner obvious. Compose small named
helpers and typed boundaries; avoid copy-pasted orchestration and anonymous
blobs of shape.

DRY up repeated mechanics when they express one domain fact. Do not hide
legitimately different behavior behind a forced abstraction; three similar
lines are better than a premature shared helper.

Refactor after the patch works. A passing diff with awkward ownership is not
done. Replace string-shaped domain values with typed constructors or
declarative helpers at the owner boundary instead of pushing parsing back into
callers.

Treat lint failures as design feedback. Suppress only for a deliberate local
invariant, a generated or external shape, or a documented tool limitation. Keep
the suppression small and explain it next to the line.

Reject fallback paths. If an owner, route, capability, config, schema, or
transport is unavailable, return a typed error and make it observable rather
than guessing a safe default. Sentinel values, silent retries, and hidden
backstops trade today's reliability for tomorrow's debugging.

Tests defend behavior or invariants. Skip tests that mirror implementation
trivia; they pin the next refactor to today's shape without protecting any
caller.
