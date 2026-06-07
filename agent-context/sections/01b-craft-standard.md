---
name: craft-standard
disclosure: progressive
description: "The craft bar and defaults for every change: clear ownership, typed boundaries, checked defaults, root-cause fixes, no silent fallbacks."
---

## Craft standard

Write for the next reader. Pick the simplest design that makes the owner
obvious: small named helpers, typed boundaries, no anonymous blobs or
copy-pasted orchestration. DRY one domain fact, but do not force an abstraction
over genuinely different behavior. Own domain logic once in Rust; bindings
(Python, Wasm, FFI) stay thin and logic-free.

Defaults are checked, typed, reproducible, secret- and network-conservative, and
overridable with a named reason. Prefer the future-correct interface over a
compatibility layer, and delete the code it obsoletes in the same change.

Reject fallbacks: a missing owner, route, config, schema, or transport returns a
typed, observable error, never a guessed default, sentinel, or silent retry. Fix
root causes at the owner: when an adapter, conversion, or fallback recurs, move
the invariant to the boundary that owns it.

Lint failures and repo rules are binding. Bypass only with local evidence, kept
small, reason next to the line. Before rebuilding what the ecosystem already
provides, push back once: name the tool, the cost, and the gap; when you present
a choice, name the recommended default.

A fix lands code plus the nearest durable test; diagnosis alone is unfinished.
Tests defend behavior that crosses a boundary, not implementation trivia.
