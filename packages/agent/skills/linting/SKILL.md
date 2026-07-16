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

Always lint through `nix run .#lint` (or build the package), never an ad-hoc
`nix shell nixpkgs#ruff -c ruff check`. The flake pins ruff and passes a fixed
`--target-version`; an ambient ruff is a different version with a different
default target, so version-gated rules (e.g. `UP041`, `PERF203`) fire in one and
not the other. A check that passes ad-hoc can still fail the gate, and vice
versa.
