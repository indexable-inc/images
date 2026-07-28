---
name: linting
description: "Running the repo lint (nix run .#lint) and that the pre-commit hook and CI share the entry point. Use before committing or when a lint check fails."
---

## Linting

```sh
nix run .#lint
```

The tracked pre-commit hook runs the same lint app. CI runs the same check
through the flake. Keep one lint entry point so local and CI failures mean the
same thing.

For machine consumption (loading results as a dataframe rather than grepping
the log), `nix run .#lint -- --json` prints one JSON array with a record per
check — `{check, ok, output}`, where `output` carries the stage's diagnostics
on failure — and still exits nonzero when any check fails.

To auto-apply the fixable findings (alejandra/statix/deadnix over `.nix`,
`ruff --fix` over `.py`), run `nix run .#lint -- --fix`: it builds the fixer
lanes as derivations and `git apply`s the resulting patch to the worktree.
Commit your work first: Nix snapshots the committed state in a linked
worktree, and `git apply` refuses on context mismatch instead of clobbering
uncommitted edits. Verdict-only stages (astlog, filenames, dirnames, clone)
and unfixable findings still require hand edits; re-run `nix run .#lint`
afterwards.

## Run every gate through nix, never an ambient tool

Always lint through `nix run .#lint` (or build the package), never an ad-hoc
`nix shell nixpkgs#ruff -c ruff check`. The flake pins ruff and passes a fixed
`--target-version`; an ambient ruff is a different version with a different
default target, so version-gated rules (e.g. `UP041`, `PERF203`) fire in one and
not the other. A check that passes ad-hoc can still fail the gate, and vice
versa.

The same holds for Rust, where the gap is wider and fails more quietly. Reach
the pinned driver through nix, either by running the gate or by putting the
repo's own clippy on PATH:

```sh
nix run .#lint
nix build .#llm-clippy --no-link --print-out-paths   # then PATH=<out>/bin:$PATH
```

A bare `cargo clippy` resolves to whatever clippy is on PATH, and this repo's
clippy is a fork carrying eleven restriction lints that stock clippy has never
heard of (`packages/llm-clippy`, `lib/fork-packages.nix`). Stock clippy does not
skip them; it raises `error[E0602]: unknown lint` and the crate never compiles,
so no lint pass runs over the code at all.

That failure is dangerous because of its shape: the run produces no findings,
and no findings looks like a pass. On 2026-07-28 an agent ran `cargo clippy`,
grepped the output for findings, found none, and reported the crate clean. The
compile had aborted. Running the forked driver over the same code found two real
violations, both in code that had already merged: a struct over the
three-bool limit and a function over the hundred-line limit.

So: check the exit code, never the absence of a string in a log. `grep -c error`
returning zero is not a pass when the exit code was 101.
