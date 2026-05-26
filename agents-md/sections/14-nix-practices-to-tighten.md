## Nix practices to tighten

Improve these patterns when touching nearby code. If cleanup is wider than the
task, file a narrow issue.

- Prefer precise option types over broad attrs. Keep broad attrs at true foreign
  format boundaries.
- Filter local sources to the smallest useful tracked file set.
- Use `lib.getExe` or `lib.getExe'` instead of spelling `${pkg}/bin/foo`
  repeatedly.
- Keep validation in shared builders and reuse those builders everywhere.
- Fix the improper layer when stricter validation exposes a helper problem.
- Use checked Nushell helpers for non-trivial generated commands.
- Keep new scripts in a language that matches the data shape they handle.
- Default to no `devShells.default`; add per-package shells or build inputs where
  the need belongs.
- Keep the tracked pre-commit hook as a small entry point to the lint app.

