#!/usr/bin/env bash
#
# Are the Rust crates lint-clean and green, in every feature configuration?
#
# ## Why this exists at all
#
# `rust/Cargo.toml`'s `[workspace.lints.clippy]` denies `unwrap_used`,
# `expect_used`, `panic` and `indexing_slicing`, mirrored from the index
# monorepo, and nothing in this fork ever ran clippy: hosted CI was removed on
# 2026-08-03, the ciCheck derivations that file's comment defers to do not
# exist here, `nix develop` has no cargo (ENG-12458, ENG-12464), and the
# pre-commit hooks in maintainers/flake-module.nix are all C++, Nix, meson and
# shell. So the deny-list was aspirational: `cargo clippy --all-targets -p
# nix-eval-rs` exited 101 with 41 errors on ix-patched at 4c02bed96.
#
# ## Why it runs the tests, and in more than one configuration
#
# Because the other half of the same hole is that `--no-default-features` was
# only ever *built*, never tested. Two perf tests asserted counter
# accumulation while the counters were compiled out and failed deterministically
# in that configuration for as long as it existed, and no one saw it, because
# "it builds" was the whole check (ENG-13005). A gate that lints one
# configuration and tests another is two partial gates.
#
# The three configurations below put each of the crate's two features in both
# states: `perf` on and `perf-ops` off (the shipped default), both off, both
# on. The fourth combination -- `perf-ops` without `perf` -- is deliberately
# not run: it is legal but nothing builds it, and the pair that
# maintainers/ix/perf-counter-overhead.md measures is the first two. Add it
# here if that stops being true.
#
# `ix-kernel` has no `[features]`, so it is run once rather than three times.
#
# ## Why one of them is a release build
#
# The three above are debug, which is what `cargo test` gives you and what
# catches the most (debug has the integer-overflow checks and `debug_assert!`
# that release drops). But the number quoted in a PR here is
# `cargo test --release`, and a gate that checks a different profile from the
# one people verify with is a gate that can be green while the quoted run is
# red. One release arm on the default configuration closes that without
# running the whole cross-product: profile is a second axis, not a multiplier
# on the first, and a feature bug is visible in either profile.
#
# ## The tests run parallel, which they did not always survive
#
# ENG-12939 made the lib suite race on process globals: a parallel run failed
# a couple of times in twenty-four with a different test named each time, and
# this gate carried `--test-threads=1` to keep three configurations from
# inheriting three times that rate. #165 gave the `Vm` its settings instead of
# reading the statics and the flag came off. Measured here on aarch64-darwin,
# `cargo test -p nix-eval-rs --lib`, parallel:
#
#   ix-patched 242e89701 (pre-#165)   20/24 pass
#   this branch (pre-#165 merge)      22/24 pass
#   this branch (post-#165 merge)     24/24 pass
#
# If that regresses, the answer is not to put the flag back: a serial gate
# cannot see a newly introduced race, which is the thing worth catching.
#
# ## Why clippy runs with `-D warnings`, and what that costs
#
# The deny-list catches four lints. Everything else clippy has to say arrived
# as a warning nobody had to act on, and 22 of them had accumulated by
# fe960c549 -- an unused import, five `assert!(false)` in tests, a doc comment
# that had come loose from the item it described and silently become the
# rustdoc for the function below it. None was a bug; the last one was a
# defect. A warning stream nobody must clear is a warning stream nobody reads.
#
# The cost is real and worth stating: the toolchain is not pinned. There is no
# `rust-toolchain.toml`, the dev shell has no cargo (ENG-12458), and this runs
# against whatever `nix shell nixpkgs#clippy` supplies, so a nixpkgs bump can
# introduce a lint and redden the gate on code nobody touched. When that
# happens the answer is to fix it or to `#[allow]` it with a reason on the
# item, not to drop the flag: an allow says which lint and why, where dropping
# the flag says nothing and takes the other twenty-one with it.
#
# ## Why formatting is checked here too
#
# `cargo fmt` had never been run over this tree: 627 hunks across 50 files by
# fe960c549 (ENG-13014). A backlog that size can only be cleared in a window
# where no other crate branch is open, because it conflicts with all of them,
# and it regrows the moment nothing is watching. `rustfmt.toml` pins
# `style_edition` so the largest source of cross-version drift is named rather
# than inherited; the binary itself is still whatever the shell has, with the
# same caveat as above.
#
# ## Not repeated here
#
# The deny-list. Repeating it is how a gate comes to check a different set
# from the one the workspace declares; clippy reads
# `[workspace.lints.clippy]` itself and this reports what it said.
#
# Two lints the index workspace also denies are absent from `rust/Cargo.toml`
# and therefore from this gate: `anonymous_tuple_return_type` and
# `fallible_int_fallback` exist only in the llm-clippy fork. Stock clippy does
# not skip a lint it has never heard of -- it raises E0602 and aborts before
# linting anything, so naming them here without the forked toolchain would
# turn this into a run with no findings, which reads exactly like a pass.
set -u

cd "$(cd "$(dirname "$0")/../.." && pwd)/rust" || exit 2

command -v cargo >/dev/null || {
  echo "rust-crate-gate: no cargo on PATH. The dev shell does not have one"
  echo "  (ENG-12458); use: nix shell nixpkgs#cargo nixpkgs#rustc nixpkgs#clippy"
  exit 2
}
cargo clippy --version >/dev/null 2>&1 || {
  echo "rust-crate-gate: cargo has no clippy subcommand; add nixpkgs#clippy"
  exit 2
}

log=$(mktemp) || exit 2
trap 'rm -f "$log"' EXIT

rc=0
summary=""

# Strip ANSI before reading a log. cargo colours its output when it thinks it
# has a terminal, and a grep for 'error' against a coloured stream matches
# nothing and returns a clean zero.
plain() { sed $'s/\033\[[0-9;]*m//g' "$log"; }

# LABEL PACKAGE [CARGO-FLAGS...]
#
# `--all-targets` on the clippy run is load-bearing: without it clippy lints
# the library only, and every one of the 41 errors this gate was written for
# was in a `#[cfg(test)]` module or an integration test. A run that skips the
# test targets reports zero and passes.
check() {
  local label=$1 package=$2; shift 2
  local errors tests failed step_rc

  # No pipe around cargo: the exit status of a pipeline is the last stage's,
  # which is how a red gate comes to print rc=0 (ENG-12444).
  cargo clippy --all-targets "$@" -p "$package" -- -D warnings >"$log" 2>&1
  step_rc=$?
  errors=$(plain | grep -cE '^error(\[|: )') || errors=0
  if [ "$step_rc" -ne 0 ]; then
    rc=1
    echo "--- clippy $package [$label]: exited $step_rc, $errors error line(s) ---"
    plain | grep -A3 -E '^error(\[|: )'
  fi

  cargo test "$@" -p "$package" >"$log" 2>&1
  step_rc=$?
  # Counted, not merely checked for absence of failures: a run that built
  # nothing and a run that passed everything both have zero failures, and the
  # difference is the whole question. `tests` below must be positive.
  tests=$(plain | grep -oE 'test result: ok\. [0-9]+ passed' | grep -oE '[0-9]+' | paste -sd+ - | bc) || tests=0
  [ -n "$tests" ] || tests=0
  failed=$(plain | grep -oE '[0-9]+ failed' | grep -oE '[0-9]+' | paste -sd+ - | bc) || failed=0
  [ -n "$failed" ] || failed=0
  if [ "$step_rc" -ne 0 ]; then
    rc=1
    echo "--- test $package [$label]: exited $step_rc, $failed failed ---"
    plain | grep -E '^(test result|    [a-z_]+::)' | head -40
  elif [ "$tests" -eq 0 ]; then
    rc=1
    echo "--- test $package [$label]: exited 0 having run no tests; refusing to call that a pass ---"
  fi

  summary+=" $package/$label=$errors/$tests"
}

# Formatting first: it is the cheapest check and a reformat touching every file
# would otherwise be reported as a wall of clippy context.
#
# Counted, not merely exit-status-checked, so "0 files need formatting" and "no
# files were examined" are different outcomes on the line below.
cargo fmt --check >"$log" 2>&1
fmt_rc=$?
fmt_hunks=$(plain | grep -c '^Diff in ') || fmt_hunks=0
if [ "$fmt_rc" -ne 0 ]; then
  rc=1
  echo "--- cargo fmt --check: exited $fmt_rc, $fmt_hunks hunk(s) unformatted ---"
  plain | grep '^Diff in ' | sed 's/^/    /' | head -20
fi
summary+=" fmt=$fmt_hunks"

# `perf` on, `perf-ops` off -- what the shipped binary links.
check default    nix-eval-rs
# Both off.
check no-perf    nix-eval-rs --no-default-features
# Both on.
check all-perf   nix-eval-rs --all-features
# No [features] of its own, so one run is every configuration it has.
check only       ix-kernel
# Likewise, and listed here for a reason worth stating: `cargo fmt --check`
# above covers the whole workspace, but clippy and the tests are per-package,
# so a new workspace member nobody adds to this list is a member nothing
# lints and nothing tests. It would still be built by everything that depends
# on it, which is what makes the omission invisible.
check only       nix-eval-driver
# The profile a PR quotes. Default features, because the point here is the
# profile and not the feature set.
check release    nix-eval-rs --release

# Printed pass or fail, and as errors/tests-passed per configuration, so a
# configuration silently dropped from the list above is visible as an absence
# from this line rather than as nothing at all.
if [ "$rc" -eq 0 ]; then
  echo "RESULT rust-crate-gate pass${summary} (fmt-hunks; clippy-errors/tests-passed)"
else
  echo "RESULT rust-crate-gate FAIL${summary} (fmt-hunks; clippy-errors/tests-passed)"
fi
exit "$rc"
