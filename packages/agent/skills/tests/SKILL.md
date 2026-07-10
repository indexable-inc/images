---
name: tests
description: "What to test and what to skip: defend behavior across boundaries, passthru.tests, delete checks that restate source. Use when adding tests or judging whether a check earns its keep."
---

## Tests

Tests should protect behavior that can regress across boundaries: module merges,
generated units, fleet rendering, artifact wiring, security posture, and runtime
contracts. Avoid asserting facts already obvious from the literal config under
test.

Image and reusable package derivations expose focused tests through
`passthru.tests.<name>`. Cross-image eval invariants live in checks. Keep
`checkPhase` or `installCheckPhase` for cheap checks that should always run with
the build.

When a change tightens source filtering, dependency identity, generated
derivations, or cache behavior, add a test that changes one small input and
proves the unrelated output remains unchanged.

### Delete checks that restate the source

Do not write, and proactively delete, checks whose only job is to re-spell a
constant that lives a few lines away. A check is restating code when changing
the source forces the same edit in the check, or when the check could only fail
if someone hand-edited it to disagree with itself. These add maintenance cost
without ruling out any real bug. Concrete shapes to remove:

- NixOS module `assertions = [ { assertion = ...; message = ...; } ]` entries
  that compare an option to the literal value the same module or a sibling file
  sets (pinned versions, dates, image tags, derivation names, enum variants
  routed through `mkDefault`).
- Flake `checks`, `passthru.tests`, or `installCheckPhase` blocks that re-grep
  a hash, version string, or filename out of a derivation that the build
  already pinned.
- Unit tests, Rust `assert_eq!`s, or Python `assert`s that compare a constant
  to itself through an indirection, mirror the function body line-for-line, or
  pin an enum's `Display` impl to its own variant names.

Keep an assertion or test when it crosses a real boundary: two files that must
agree but have no shared source of truth, a generated artifact that must match
a manifest, a runtime invariant that the type system cannot express, or a
regression you can name with a failing reproduction. If the failure mode you
are guarding against is "someone edited both halves to lie in unison," the
check is not earning its keep.

Fix the root cause instead. When two places must agree, route both through one
binding, one option default, or one generated value, and let the type checker
or module merge enforce the link.

### Before writing an assertion, run the failure test

Ask: if this assertion failed, would it reveal a bug a reader could not predict
from the source line it checks? Or would it only fire when someone deliberately
edits that exact literal? Write it only in the first case.

- Only assert genuinely useful, non-obvious behavior that a reader cannot
  trivially derive from the source under test.
- Do not assert a literal constant against itself (a date, tag, name, port, or
  retention count round-tripped through `fromJSON`/`toJSON`), and do not assert
  what the type system or `builtins.toJSON` already guarantees cannot malform.
- A real invariant earns the line: a security or policy property, a required
  package's presence, two files that must agree with no shared source, a
  generated artifact that must match a manifest, or parser/round-trip behavior
  with a genuine failure mode.
