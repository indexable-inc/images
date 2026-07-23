---
name: property-testing
description: "Property-based testing with hegel (Antithesis's Hypothesis-based library): which properties to reach for, generator patterns, shrinking, replay, CI determinism. Use when adding property tests, fuzzing a parser/converter/codec, or a test needs generated rather than example inputs."
---

## Property testing with hegel

Repo-owned Rust crates use `hegeltest` (workspace dev-dependency; lib name
`hegel`). It is Antithesis's Hypothesis-based property-testing library:
pure-Rust engine, `#[hegel::test]` under plain `cargo test`, automatic
shrinking to a minimal counterexample, and a failure database (`.hegel/`,
gitignored) that replays the last counterexample first on the next run.
Reference suites: `packages/ix2nix/tests/properties.rs`,
`packages/minecraft/nbt/tests/property.rs`.

## Which properties earn their keep

In rough order of value per line:

1. **Never panics**: feed `gs::text()` (or arbitrary bytes) to any parser,
   decoder, or converter; assert it returns `Ok`/`Err` rather than
   panicking. Cheapest test, catches the most.
2. **Round trip**: serialize then reparse (or encode/decode) and compare.
   For a transpiler the analogue is: generated well-formed source converts,
   and the output parses cleanly under an independent parser (ix2nix uses
   `rnix` as its oracle).
3. **Determinism / idempotence**: same input twice gives identical output;
   `f(f(x)) == f(x)` where claimed.
4. **Model-based**: compare against an obviously-correct implementation
   (BTreeMap vs your map; a slow interpreter vs your fast path).

## Generator patterns

- Recursive structures: `gs::deferred::<T>()` + `handle = node.generator()`
  + `node.set(hegel::one_of!(...))`; bound growth with `.max_size(n)` on
  `gs::vecs`, or the tree can explode.
- `gs::sampled_from` takes a slice (`&[..][..]`), not an array.
- Keep generated programs *well-formed by construction* (fixed distinct
  binder names, deduplicated object keys) and let values carry the
  randomness; otherwise the property tests your generator, not the code.
  Two real generator traps from ix2nix: `${...}` interpolation only exists
  inside backtick templates, and `=> {}` is a block, not an object literal.
- A composite generator is a function returning `impl Generator<T>` built
  from `.map` over `hegel::tuples!(...)`; `#[hegel::composite]` exists for
  the draw-style equivalent.

## Budget, replay, CI

- Default is 100 test cases; override with `#[hegel::test(test_cases = N)]`.
  Keep CI at the default; run big budgets ad hoc.
- Runs are randomized per invocation, so CI is a rolling fuzzer: a red main
  property test means hegel found a real counterexample that was always
  legal input. Fix forward; never re-run to green. The failure database
  gives local replay, but CI sandboxes start fresh, so copy the printed
  `let source = ...` shrunk case into a regression `#[test]` alongside the
  fix (see `bool_and_boolean_both_lower` in ix2nix).
- The engine and macros come from crates.io (`hegeltest`,
  `default-features = false`); no Python, no network at test time, so tests
  run unchanged inside nix sandboxes.

## Antithesis

Hegel is Antithesis's PBT family (hegel.dev): the same tests can later run
under the Antithesis platform for guided exploration, and the `antithesis`
feature wires assertions into its SDK. Do not enable that feature for
ordinary CI; it is for workloads launched via the antithesis-* skills.
