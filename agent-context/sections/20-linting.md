---
name: linting
disclosure: progressive
description: "Running the repo lint (nix run .#lint) and that the pre-commit hook and CI share the entry point. Use before committing or when a lint check fails."
---

## Linting

```sh
nix run .#lint
```

The tracked pre-commit hook runs the same lint app. CI runs the same check
through the flake. Keep one lint entry point so local and CI failures mean the
same thing.
