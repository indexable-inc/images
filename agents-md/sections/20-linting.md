## Linting

```sh
nix run .#lint
```

The tracked pre-commit hook runs the same lint app. CI runs the same check
through the flake. Keep one lint entry point so local and CI failures mean the
same thing.
