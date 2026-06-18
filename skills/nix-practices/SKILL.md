---
name: nix-practices
description: "Nix patterns to improve when touching nearby code: precise types, source filtering, getExe, mkPackageOption/mkEnableOption, follows. Use when writing or refactoring Nix."
---

## Nix practices to tighten

Improve these patterns when touching nearby code. If cleanup is wider than the
task, file a narrow issue.

- Prefer precise option types over broad attrs. Keep broad attrs at true foreign
  format boundaries.
- Filter local sources to the smallest useful tracked file set.
- Use `lib.getExe` or `lib.getExe'` instead of spelling `${pkg}/bin/foo`
  repeatedly. `getExe` reads `meta.mainProgram`; reach for `getExe' pkg "name"`
  when the package ships multiple binaries.
- Keep validation in shared builders and reuse those builders everywhere.
- Fix the improper layer when stricter validation exposes a helper problem.
- Use checked Nushell helpers for non-trivial generated commands.
- Keep new scripts in a language that matches the data shape they handle.
- Avoid generated `nix run` wrappers that call `nix run`, `nix build`, or
  `nix flake check` internally. Model dependencies as derivation inputs or keep
  orchestration outside Nix.
- Default to no `devShells.default`; add per-package shells or build inputs where
  the need belongs.
- Keep the tracked pre-commit hook as a small entry point to the lint app.
- Use `stdenv.mkDerivation (finalAttrs: { ... })` over `let version = ...; in
  mkDerivation { inherit version; ... }`. `finalAttrs:` is the canonical
  self-reference (we ban `rec`), and overrides propagate cleanly.
- Use `lib.mkPackageOption pkgs "<name>" { }` instead of hand-rolled
  `package = mkOption { type = types.package; default = pkgs.<name>; };`. It
  produces consistent `defaultText`, `example`, and the `nullable`
  no-install path for free.
- Use `mkEnableOption "<noun>"` instead of `mkOption { type = types.bool;
  default = false; description = "Whether to enable ..."; }`. Pass a bare noun
  phrase; the helper renders `"Whether to enable <noun>."` itself.
- When a `default` references `pkgs.*`, `cfg.*`, or any non-literal expression,
  set `defaultText = lib.literalExpression "..."` so the generated option docs
  show the expression instead of the resolved store path.
- Use the standard meta block: `meta.description`, `meta.license` (typed),
  `meta.mainProgram` when the derivation ships a binary, and a `passthru.tests`
  entry for cheap smoke tests.
- Use Markdown roles in option descriptions where they sharpen meaning:
  `{file}`, `{option}`, `{command}`, `{env}`, `{manpage}`. The renderer
  consumes them; even without rendering, they encode intent.
- Keep transitive `inputs.<x>.inputs.nixpkgs.follows = "nixpkgs"` set on every
  flake input that itself takes a nixpkgs. Each unfollowed nixpkgs duplicates
  the evaluator's working set.
- Cross-compilation hygiene: `cmake`, `meson`, `ninja`, `pkg-config`,
  `autoreconfHook`, `makeWrapper`, `wrapGAppsHook` all belong in
  `nativeBuildInputs`, never `buildInputs`. The splicing in `buildInputs`
  swaps them to the target platform and silently breaks the build.
