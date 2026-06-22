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

Always lint through `nix run .#lint` (or build the package), never an ad-hoc
`nix shell nixpkgs#ruff -c ruff check`. The flake pins ruff and passes a fixed
`--target-version`; an ambient ruff is a different version with a different
default target, so version-gated rules (e.g. `UP041`, `PERF203`) fire in one and
not the other. A check that passes ad-hoc can still fail the gate, and vice
versa.
