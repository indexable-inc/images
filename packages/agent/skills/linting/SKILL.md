---
name: linting
description: "Running each repository's authoritative lint gate. Use before committing or when a lint check fails."
---

## Linting

In `index`, run:

```sh
nix run .#lint
```

Its tracked pre-commit hook and CI use the same lint app.

For machine consumption, `nix run .#lint -- --json` prints one JSON array with
a `{check, ok, output}` record per check and still exits nonzero when any check
fails. `output` carries each failed stage's diagnostics.

In `ix`, build the system-qualified Linux package that its CI lint phase uses:

```sh
nix build -L --no-link .#packages.x86_64-linux.ci-lint-checks
```

The explicit system is intentional on Darwin: the configured remote builder
executes the Linux gate.

Do not replace either gate with ambient tools such as
`nix shell nixpkgs#ruff -c ruff check`. The flakes pin their tools and flags, so
an ad hoc check can disagree with the repository gate.
