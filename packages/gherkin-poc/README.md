# gherkin-poc

A worked example of two techniques, kept deliberately tiny: Gherkin
(behavior-driven) tests via [cucumber-rs], and a [cargo-mutants] audit of how
well those tests pin the behavior down. Tracked in
[indexable-inc/index#4091](https://github.com/indexable-inc/index/issues/4091).

The domain is an `Account` holding integer cents with an agreed overdraft.
The behavior lives in `tests/features/account.feature` as plain-language
Given/When/Then scenarios; `tests/gherkin_account.rs` binds each step to the
crate API. The suite runs under the normal libtest harness (cucumber's
suggested `harness = false` only affects output ordering), so nextest and
cargo-unit treat it like any other test:

```sh
cargo test --package gherkin-poc
```

## Mutation audit

Per the rust-style skill, cargo-mutants is a package-owner audit tool, not a
CI gate. The recorded run (cargo-mutants 27.1.0, 2026-07-23):

```sh
nix shell nixpkgs#cargo-mutants -c cargo mutants --package gherkin-poc --cap-lints=true
# 19 mutants tested in 33s: 18 caught, 1 unviable
```

Notes from the audit, in the order they happened:

- Without `--cap-lints=true` the workspace's `warnings = "deny"` makes whole
  function-body replacements (`withdraw -> Ok(())`) fail to compile on unused
  parameters, so they report as unviable instead of being exercised. Cap
  lints to actually test them.
- The first capped run left one survivor: replacing
  `Display for AccountError` with an empty write, because no scenario
  asserted error text. The feature gained
  `And the error reads "..."` steps; the survivor is now caught. That
  tighten-the-spec loop is the point of pairing the two tools.
- The one remaining unviable mutant replaces `const fn with_overdraft` with
  `Default::default()`, which cannot compile in a const context: a
  write-off, not a gap.

[cucumber-rs]: https://github.com/cucumber-rs/cucumber
[cargo-mutants]: https://mutants.rs/
