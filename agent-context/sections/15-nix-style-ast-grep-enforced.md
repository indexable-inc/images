---
name: nix-style
disclosure: progressive
description: "Nix code style, general and mechanical: a design and idiom checklist for new code, plus the rules enforced by ast-grep and lint (no with/rec/mkForce, derivation hygiene, typed options, fetchers, licenses). Use when writing or reviewing Nix, or fixing a lint failure."
---

## Adding a new Nix thing

Craft-standard governs ownership, DRY, layering, consumers, and not being
defensive. The Nix-specific checks on top:

- Functional over imperative: index-driven iteration (`imap0`, `lib.range`,
  `elemAt`) usually means a missing `map`, `genList`, or higher-level combinator.
- Reuse `builtins` and `nixpkgs.lib` before hand-rolling; an experienced Nix
  reader should have no complaints on first read.
- Reify latent structure, parse rather than validate, and leave room for future
  needs in the design without writing dead code.

## Nix style (ast-grep enforced)

Run `nix run .#lint` before committing. It runs nixfmt, Statix, Deadnix, and the
repo's ast-grep rules. The lint app is the mechanical source of truth. The
common hard rules are:

### Scope / access

- No `with pkgs;` / `with lib;` / `with builtins;`. Use `inherit (pkgs) ...` or
  `lib.foo` directly.
- No `pkgs.lib.X`. Bind `lib` in the function signature (or `inherit (pkgs)
  lib;` once at the top of a `let`) and use `lib.foo` everywhere.
- No `rec { }` and no `let { ... }` legacy form. Use `let ... in` or
  `finalAttrs:` for mkDerivation self-reference.
- No `mkForce` and no `mkOverride <int>` back-door. Fix the module boundary or
  compose `mkDefault` / `mkOptionDefault` / `mkBefore` / `mkAfter`.
- No `lib.recursiveUpdate`. Build the attrset in one place or use `lib.mkMerge`.
- No `{ } // X` / `X // { }` attrset updates with an empty operand.
- No `mkMerge [ x ]` single-element wrappers; drop the wrapper.
- No repeated parent keys in the same attrset. Group related assignments under
  one parent.
- Prefer `inherit (source) name;` for direct same-name field copies.

### Eval and source paths

- No `builtins.currentSystem`, `builtins.getEnv`, `<nixpkgs>`, or `path:` flake
  refs. No `builtins.getFlake (toString ./...)`.
- No `(import ./foo.nix)` inside `imports = [ ... ]`; NixOS auto-imports paths.
- No `..` paths inside `modules/`; shared helpers come through `specialArgs.ix`.
- `builtins.path { path = ./.; }` must set `name = "<stable>"` so the store
  path is reproducible across clones.
- Prefer `lib.fileset.toSource` over `lib.cleanSource`/`lib.sources.cleanSourceWith`.
- No `"${root}/..."` string interpolation of the workspace tree at the root
  level; use `root + "/..."` or `builtins.path { name; path; }`.

### Migration / deprecated APIs

- No `lib.mdDoc` / `lib.options.mdDoc` / bare `mdDoc`. Pass plain Markdown.
- No `substituteAll` / `substituteAllFiles` (removed from nixpkgs). Use
  `pkgs.replaceVars` / `replaceVarsWith`.
- No `cargoSha256` (use `cargoHash` or `cargoLock`), no `vendorSha256` (use
  `vendorHash`), no `npmDepsSha256` (use `npmDepsHash`). `pnpmDepsHash` is the
  current name on the pnpm side and is not flagged.
- No bare `buildRustPackage`; use `pkgs.rustPlatform.buildRustPackage` or
  `crane.buildPackage`.
- No `flake-utils.lib.eachSystem`; we hand-roll per-system in
  `lib/per-system.nix`.

### Idioms (mechanical)

- Use `lib.importJSON path` / `lib.importTOML path` instead of
  `builtins.fromJSON (builtins.readFile path)`.
- Use `(pkgs.formats.json { }).generate "name" value` instead of
  `pkgs.writeText "name" (builtins.toJSON value)`.
- Use `lib.optional cond x` (singular) when the conditional yields one element;
  reserve `lib.optionals cond xs` (plural) for actual lists.
- Use `lib.genAttrs keys f` instead of `lib.listToAttrs (map (n: { name = n;
  value = f n; }) keys)`.
- Use `builtins.attrValues X` instead of `lib.mapAttrsToList (_: v: v) X`.
- Use `lib.genAttrs (lib.attrNames X) (_: v)` instead of
  `lib.mapAttrs (_: _: v) X` when both arguments are discarded.
- Use `xs == [ ]` / `xs != [ ]` instead of `builtins.length xs == 0` / `> 0`.
- No `!true` / `!false` literals; write the inverse literal directly.
- No `mkIf true x` / `lib.optional true x`; constant conditions on these
  helpers are refactor leftovers.
- No `name = "${pname}-${version}"` restatement; stdenv constructs `name` from
  `pname` + `version`. (Use `pname` + `version` instead of a single dashed
  `name` so updaters and `meta` rendering can parse the version.)
- Wrap dynamic attrpath antiquotes: `legacyPackages."${system}"`, not
  `legacyPackages.${system}`.

### Derivations / mkDerivation

- Set `strictDeps = true` on every `mkDerivation`.
- No `fixupPhase = ...` override; use `preFixup` / `postFixup`. Same idea for
  `buildPhase` / `installPhase` — do not restate the stdenv defaults.
- `configureFlags` / `cmakeFlags` / `mesonFlags` / `makeFlags` / `ninjaFlags`
  are lists of strings; never one string with spaces.

### Types and options

- No `types.attrs` / `lib.types.attrs` / `types.unspecified` for public
  options. Use a typed `submodule` with `freeformType = (pkgs.formats.<x> {}).type`,
  or an explicit `oneOf` / `attrsOf <type>`.
- `mkOption.default` should be a self-contained expression. Conditional
  defaults that branch on sibling cfg belong in `config = ...` with `mkDefault`.

### Hashes / licenses / fetchers

- Keep raw fetched data artifact URLs out of `flake.nix`.
- Use `pkgs.*` fetchers instead of `builtins.fetch*`. Put the hash in the SRI
  `hash` slot (`hash = "sha256-...="`), never a legacy `sha256 = ...` attr. The
  fetcher `sha256` ban and the removed `cargoSha256` / `vendorSha256` /
  `npmDepsSha256` builder attrs are one rule (`prefer-sri-hash`, error). The
  only exception is a hex digest that must round-trip verbatim (e.g. a
  Cargo.lock checksum fed back into `.cargo-checksum.json`, which cargo verifies
  as hex): keep it on `sha256` and add a `# ast-grep-ignore: prefer-sri-hash`
  comment naming the reason.
- Commit real hashes, never fake hash helpers or placeholders.
- `meta.license` should reference `lib.licenses.<id>`, never a raw SPDX
  string. The bare `gpl2` / `gpl3` / `lgpl2` / `lgpl3` / `agpl3` aliases are
  banned by ast-grep — pick the explicit `*Only` / `*Plus` flavor
  (`agpl3Plus`, not `gpl3Plus`, when the upstream is AGPL).

### Errors and warnings

- No bare `assert cond;`. Use `assert lib.assertMsg cond "...";`.
- No `abort`. Prefer `throw "ix.<area>: ..."` (catchable) or
  `lib.assertMsg` for invariants. `throw ""` is the same shape as a bare
  assert and is rejected.
- No leftover `builtins.trace` / `lib.traceVal` / `lib.traceSeq` in tracked
  code.

### Build / configuration outputs

- No `writeShellApplication` or `writeShellScriptBin` for user-facing commands.
- No unused bindings. Use `_` for intentionally unused lambda arguments.
- Use `nixosModules.<name>` for module exports. Avoid a flat top-level
  `modules` output.
- Keep image targets at `x86_64-linux`.
- Use structured config options for new modules instead of stringly config
  fragments.
