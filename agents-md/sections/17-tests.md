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

