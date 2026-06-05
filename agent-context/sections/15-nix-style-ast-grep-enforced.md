---
name: nix-style
disclosure: progressive
description: "Nix code style, general and mechanical: a design and idiom checklist for new code, plus the rules enforced by ast-grep and lint (no with/rec/mkForce, derivation hygiene, typed options, fetchers, licenses). Use when writing or reviewing Nix, or fixing a lint failure."
---

## So you're about to add a new Nix thing

### STOP!

Check these first:
- Is your code **idiomatic**?
- Is your code **elegant**?
- Is your code **needed** and **useful**?
  - Are there any consumers of your new abstraction or is it DOA?
  - If you do not need something but it might be wanted in the future, _leave room for it_ in the design, but don't write dead code.
- Are you writing imperative code rather than using **functional programming as your guiding philosophy**?
  - Common code smells like index-driven iteration (often using `imap0`, `lib.range`, `elemAt`, for example) usually signal a missing `map`, `genList`, etc.
  - Prefer higher-level combinators over composition of many low-level structures.
- Are you **reimplementing** code that is already in builtins or nixpkgs.lib? Could you write simpler code if you used existing functions?
- Does your code **make sense**? What would it be like for an experienced Nix dev to see it for the first time? Would they have any complaints? If so, address them.
- Are you **repeating yourself**? Are you writing the same construct over and over?
  - Consider factoring when repetition obscures intent or makes changes error-prone.
  - As a heuristic, it may be time to add an abstraction if you're writing the same line 3+ times, or a 3+ line block at least twice.
  - Readability is the deciding factor.
- Are you **layering** properly?
  - Your function hierarchy should mirror the conceptual hierarchy and the domain.
  - Helper functions should make sense as a self-contained unit; fracturing an overly large tree is not a valid way of making it easier to understand.
    - If a helper function is only used once, consider making it a `let`/`in` binding or inlining it.
- Are you adding **special cases** or exceptions that aren't a natural part of your domain?
  - That often means something is wrong at a deeper level.
  - Refine the overarching structure instead.
  - If you're making exceptions just because other parts of the codebase use your code, consider just refactoring the callers - this often makes things much simpler.
- Who are the **consumers** of your code?
  - If your function is only ever used in the same file or within the repo, *do not worry about keeping its API compatible;* refactor the caller along with it.
- Is your code **overly defensive**?
  - Trust your own code. Don't add checks everywhere to values you pass around yourself.
  - Your system should be coherent instead of constantly doubting itself.

### Maxims

- Reify latent structure
- Don't over-abstract
- Parse rather than validate

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
- Use `pkgs.*` fetchers instead of `builtins.fetch*`. Prefer SRI in the
  `hash` slot (`hash = "sha256-...="`); never `sha256 = ...` in fetchers.
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
