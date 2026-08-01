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

### Watch the guard fail, and expect the break itself to be wrong

After adding an assert, a lint rule or a CI check, break the thing it protects,
confirm it fires with the message you intended, then restore. Six of the
twenty-six break-checks run across one night were silent on the first attempt.

The instinct on a silent break is that the guard is broken. That is the least
common outcome of the four. In order of how surprising they are:

**The break was invalid.** Two of the six. One changed the header string a
reader expects but not the one the writer emits, so no record ever matched,
every load started fresh, and the test passed for a reason unrelated to the
guard. Another renamed a constant that was simultaneously the recorded value,
the registry key and the expected value, so renaming it consistently is a valid
refactor and silence was correct. Ask what the real failure mode is and break
that instead: for the constant it was the recorded name and the registry key
disagreeing, so break only the one that records.

**The test cannot fail.** Three of the six, and the subtlest. One checked out
the root commit, whose tree is the empty tree, which the initializer had already
stored, so a checkout that did nothing at all passed. One asserted a path was
present in a tree that an earlier step had already put it in, so it could not
tell an update from a no-op. One asserted a carriage return survives a round
trip using a name with the `\r` in the middle, where it survives unescaped;
only a name *ending* in one is corrupted, because `str::lines` strips a trailing
`\r` as part of a `\r\n` pair.

The fix is general enough to apply by default: assert up front that the fixture
differs from the starting state, so the test cannot pass by accident.

```rust
assert_ne!(
    commit.tree().tree_ids(),
    workspace.working_copy().tree()?.tree_ids(),
    "the fixture must differ from the starting tree or the test cannot fail"
);
```

**The code is unearned.** One of the six, and the most valuable. A comparison
short-circuited when a new value equalled the old one, with a comment claiming
that was what kept two commands agreeing. Breaking it was silent. The test was
then fixed so it could fail, and it was silent again. That second silence is the
signal: the store is content-addressed, so writing the bytes a path already had
yields the same id and the same tree with or without the check. It was an
unmeasured optimization justified by a property it did not provide. The answer
is to delete the code and correct the comment, not to write a cleverer test.

**The guard is broken.** The case the rule is usually written for, and the one
you were expecting.

Two related shapes outside tests. An assertion placed where it will never be
forced, such as inside a Nix list element nothing evaluates. And a check whose
passing state is an absence: "no failures", "nothing pending", "no unmatched
rows" are all satisfied by a channel producing nothing, and report success
loudest when they are broken. A CI monitor reported "all terminal, zero
failures" over a run whose required job had been cancelled mid-suite, having
reached none of the tests it was meant to gate. Count the named things you
require and assert each one present and successful.
