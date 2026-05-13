# Contributing

Run the repo lint before pushing:

```sh
nix run .#lint
```

It checks Nix formatting (nixfmt), Statix, Deadnix, and the repo's ast-grep rules. CI runs the same derivation as a flake check.

The repo ships a tracked git pre-commit hook at `.githooks/pre-commit` that calls the lint app. To activate it locally, `direnv allow` in the repo root: `.envrc` exports `core.hooksPath` so git uses the tracked hook. No additional shell or framework is needed.

There is no `devShells.default` to enter for routine work. Reach for the per-package shell when you need build dependencies for a specific artifact, e.g.

```sh
nix develop .#minestom-hello-server-jar   # gives gradle + JDK 25
nix develop nixpkgs#nixfmt                # nixfmt + its deps
```
