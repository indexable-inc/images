# Nix ASTLog lints

Complete reference for every house-style lint ASTLog enforces on Nix source in this
repo. Generated from [`astlog-rules/nix.astlog`](./nix.astlog), the single source of
truth. **94 lints total: 90 `error`, 4 `warning`.**

## How it works

- Rules live in `astlog-rules/nix.astlog`. Each is one or more `(rule (<id> ...))`
  relations over the tree-sitter Nix grammar, plus a `(lint <id> <severity> "<message>")`
  declaration that turns every matched row into a located CI finding.
- `astlog scan` (wrapped as `astlog-scan` in the dev shell, run from the pre-commit hook
  and gated in CI) reports them. `error` fails the gate; `warning` is advisory.
- Suppress a single legitimate exception with an `astlog-ignore: <rule-id>` comment on
  the finding's line or the line directly above it.
- Every rule ships a `astlog-rules/tests/<rule-id>/{good,bad}.fixture` pair; the
  `astlog-nix-rules` flake check asserts each lint fires on `bad` and stays silent on
  `good`. The bad/good snippets below are copied verbatim from those fixtures.

## Summary

| # | Rule | Sev | Catches |
|---|------|-----|---------|
| 1 | [`no-builtins-current-system`](#no-builtins-current-system) | err | builtins.currentSystem is impure; pass system explicitly via flake or function args |
| 2 | [`no-builtins-fetch`](#no-builtins-fetch) | err | builtins.fetch* runs on eval, is not a fixed-output derivation, and does not substitute; use the matching pkgs.* fetcher |
| 3 | [`no-builtins-get-env`](#no-builtins-get-env) | err | builtins.getEnv is impure; pass values explicitly via function args or flake inputs |
| 4 | [`no-impure`](#no-impure) | err | __impure = true: prefer ix.writeNushellApplication for side-effect orchestrators; __impure is only for network side effects that must live inside the derivation graph |
| 5 | [`no-allow-builtin-fetch-git`](#no-allow-builtin-fetch-git) | err | allowBuiltinFetchGit fetches Cargo git dependencies during evaluation; pin rustPlatform.importCargoLock outputHashes instead |
| 6 | [`no-builtins-path-without-name`](#no-builtins-path-without-name) | err | `builtins.path` without `name` derives the store-path name from the working directory; set `name = "<stable>"` so the result is reproducible across clones |
| 7 | [`no-getflake-tostring`](#no-getflake-tostring) | err | `builtins.getFlake (toString ./...)` uses path-flake semantics, copying the entire tree including non-regular files; use `"git+file:///..."` instead |
| 8 | [`no-nixpkgs-channel-ref`](#no-nixpkgs-channel-ref) | err | `<nixpkgs>` channel reference is banned; use flake inputs |
| 9 | [`no-path-flake-ref`](#no-path-flake-ref) | err | whole-tree `path:` flake refs copy the working tree byte-for-byte; use `.#...`, `git+file:///abs/path`, a relative `path:./<subtree>` input, or a proper flake input |
| 10 | [`no-parent-path`](#no-parent-path) | err | relative parent path `../` reaches across a directory; use ix.<helper> / index.lib, ix.paths.<root> + a relative string, or the package registry instead of a `../` literal |
| 11 | [`no-root-string-interp`](#no-root-string-interp) | err | `"${root}/..."` string-interpolates the workspace tree, leaking full-tree context; use `root + "/..."` (path concat) or `builtins.path { name; path; }` |
| 12 | [`no-fake-hash`](#no-fake-hash) | err | fake hashes are banned; compute the real SRI hash before editing tracked Nix files |
| 13 | [`no-fetchfromgithub-fixed-hash`](#no-fetchfromgithub-fixed-hash) | err | do not pin GitHub source with `fetchFromGitHub { ... hash = ...; }`; add it as a flake input with `flake = false` and consume that source instead |
| 14 | [`prefer-sri-hash`](#prefer-sri-hash) | err | legacy `sha256`-flavored hash attr; use the SRI `hash` slot: `hash` in fetchers, `cargoHash` / `vendorHash` / `npmDepsHash` in the language builders (or the typed `cargoLock` block for Rust) |
| 15 | [`mkderivation-missing-strict-deps`](#mkderivation-missing-strict-deps) | err | mkDerivation without strictDeps = true; add it to prevent build/host dependency leaking |
| 16 | [`no-buildphase-pure-default`](#no-buildphase-pure-default) | err | `buildPhase = "make"` / `installPhase = "make install"` restates the stdenv default; drop the override or document the deviation |
| 17 | [`no-buildrustpackage`](#no-buildrustpackage) | err | `buildRustPackage` is the legacy entry point; use `rustPlatform.buildRustPackage` (typed `cargoLock` shape) or `crane` |
| 18 | [`no-derivation-name-version-interp`](#no-derivation-name-version-interp) | err | `name = "${pname}-${version}"` restates stdenv; set `pname` and `version`, and stdenv constructs `name` automatically |
| 19 | [`no-fixup-phase-override`](#no-fixup-phase-override) | err | `fixupPhase = ...` wipes the default fixup pass; use `preFixup` / `postFixup` instead |
| 20 | [`no-pname-finalattrs-self-ref`](#no-pname-finalattrs-self-ref) | err | `finalAttrs.pname` as the value of `repo`, `owner`, or `pname` itself is usually a copy-paste artifact; use the literal |
| 21 | [`no-stringly-flags`](#no-stringly-flags) | err | build flag attrs are list-of-string, not one string with spaces; whitespace inside flag values breaks paths-with-spaces and shell quoting |
| 22 | [`no-substitute-all`](#no-substitute-all) | err | `substituteAll` / `substituteAllFiles` was removed from nixpkgs; use `replaceVars` or `replaceVarsWith` instead |
| 23 | [`runcommand-missing-structured-attrs`](#runcommand-missing-structured-attrs) | warn | runCommand with empty attrs {}; add { __structuredAttrs = true; } for consistent derivation behavior |
| 24 | [`no-handrolled-shell-script`](#no-handrolled-shell-script) | warn | `writeText "*.sh"` hand-rolls an unchecked shell script; prefer a compiled Rust launcher (ix.rustWorkspace, see packages/config-launch / packages/agent/codex) or ix.writeNushellApplication / ix.writePythonApplication, or keep it with an `astlog-ignore: no-handrolled-shell-script` comment + reason |
| 25 | [`no-nix-in-generated-shell`](#no-nix-in-generated-shell) | err | Nix-generated shell applications must not call nix again; pass realized store paths into the script instead |
| 26 | [`no-write-shell-application`](#no-write-shell-application) | err | writeShellApplication is banned; use ix.writeNushellApplication instead |
| 27 | [`no-write-shell-script`](#no-write-shell-script) | err | writeShellScript is unchecked (no shellcheck, no declared deps); prefer a compiled Rust launcher/tool (ix.rustWorkspace, see packages/config-launch / packages/claude-hooks), else ix.writeNushellApplication / ix.writePythonApplication for logic, else ix.writeBashApplication (checked) for must-be-bash |
| 28 | [`no-write-shell-script-bin`](#no-write-shell-script-bin) | err | writeShellScriptBin is banned; use ix.writeNushellApplication instead |
| 29 | [`no-mkforce`](#no-mkforce) | err | mkForce is banned; use priority composition (mkDefault, mkOverride) instead |
| 30 | [`no-mkoverride-numeric`](#no-mkoverride-numeric) | err | `mkOverride <literal>` is the back-door for `mkForce`; use module priority composition (`mkDefault`, `mkOptionDefault`, `mkBefore`, `mkAfter`) or fix the module boundary |
| 31 | [`no-mkoption-conditional-default`](#no-mkoption-conditional-default) | err | a conditional `mkOption.default` couples the declaration to a sibling option; keep `default` self-contained and move the branch into `config` as `mkIf <cond> (mkDefault ...)` |
| 32 | [`no-mkif-true`](#no-mkif-true) | err | `mkIf true x` is `x` and `mkIf false x` is empty; drop the wrapper |
| 33 | [`no-mkmerge-singleton`](#no-mkmerge-singleton) | err | `mkMerge [ x ]` is identity; drop the wrapper |
| 34 | [`no-types-attrs`](#no-types-attrs) | err | `types.attrs` is the give-up type; use a typed `submodule` with `freeformType` (or `attrsOf <real-type>`) so options merge, document, and validate |
| 35 | [`no-types-unspecified`](#no-types-unspecified) | err | `types.unspecified` accepts anything, including bottom; use a real type or `attrsOf <type>` / `oneOf [ ... ]` |
| 36 | [`no-mddoc`](#no-mddoc) | err | `lib.mdDoc` / `mdDoc` is a no-op and was removed from nixpkgs; pass the description string directly |
| 37 | [`no-import-in-imports`](#no-import-in-imports) | err | use `imports = [ ./foo.nix ]`, not `imports = [ (import ./foo.nix) ]`; NixOS modules auto-import paths |
| 38 | [`no-abort`](#no-abort) | err | `abort` is a hard abort with no recovery; use `throw` (catchable) or `lib.assertMsg` (typed assertion) instead |
| 39 | [`no-bare-assert`](#no-bare-assert) | err | bare `assert <cond>;` gives an opaque failure; use `assert lib.assertMsg <cond> "<why>";` (or `lib.assertOneOf`) so the diagnostic names the invariant |
| 40 | [`no-throw-without-message`](#no-throw-without-message) | err | `throw ""` is an empty placeholder message; pass a message that names the failing invariant |
| 41 | [`no-deprecated-trace`](#no-deprecated-trace) | err | `builtins.trace` left in tracked code prints during every eval; remove it or guard behind a debug flag |
| 42 | [`no-with-builtins`](#no-with-builtins) | err | `with builtins;` is banned; use explicit builtins.foo qualifications |
| 43 | [`no-with-lib`](#no-with-lib) | err | file-scope `with lib;` is banned; use explicit lib.foo qualifications |
| 44 | [`no-with-pkgs`](#no-with-pkgs) | err | file-scope `with pkgs;` is banned; use explicit pkgs.foo qualifications |
| 45 | [`no-pkgs-lib`](#no-pkgs-lib) | err | `pkgs.lib.X` reaches through `pkgs`; use the bare `lib` binding (or `inherit (pkgs) lib;`) |
| 46 | [`no-builtin-length-list-zero`](#no-builtin-length-list-zero) | err | `builtins.length xs == 0` is `xs == [ ]` (and `> 0` is `xs != [ ]`); drop the function call |
| 47 | [`no-chained-attrset-update`](#no-chained-attrset-update) | err | `A // { ... } // { ... }` builds one attrset in two steps; merge the adjacent literals into a single `// { ... }` |
| 48 | [`no-deprecated-iflist-empty`](#no-deprecated-iflist-empty) | err | `if cond then [ x ] else [ ]` is `lib.optional cond x` |
| 49 | [`no-double-paren`](#no-double-paren) | err | `(($X))` is double-grouped; drop one layer of parens |
| 50 | [`no-empty-list-concat`](#no-empty-list-concat) | err | `[ ] ++ X` and `X ++ [ ]` are no-ops; drop the empty operand |
| 51 | [`no-negate-bool-literal`](#no-negate-bool-literal) | err | `!true` / `!false` is the opposite literal; use the literal directly |
| 52 | [`no-optional-true`](#no-optional-true) | err | `lib.optional true x` is always `[ x ]` and `lib.optional false x` is always `[ ]`; inline the literal |
| 53 | [`no-update-empty-set`](#no-update-empty-set) | err | `X // { }` and `{ } // X` are no-ops; drop the empty operand |
| 54 | [`no-unquoted-splice`](#no-unquoted-splice) | err | `legacyPackages.${system}` interpolates outside a string; prefer `import nixpkgs { inherit system; }`, or quote the antiquote: `legacyPackages."${system}"` |
| 55 | [`no-legacy-let-block`](#no-legacy-let-block) | err | `let { ... }` is the undocumented legacy let form; use `let ... in` or a normal attrset |
| 56 | [`no-rec-attrset`](#no-rec-attrset) | err | `rec { }` is banned in derivations and overlays; use let, finalAttrs:, or final/prev |
| 57 | [`no-ambiguous-gpl-license`](#no-ambiguous-gpl-license) | err | ambiguous GPL/AGPL/LGPL license identifier; use the `-Only` / `-Plus` flavor: `gpl2Only`, `gpl3Plus`, `agpl3Only`, etc. |
| 58 | [`no-flake-utils-eachsystem`](#no-flake-utils-eachsystem) | err | `flake-utils.lib.eachSystem` is discouraged; use `flake-parts` (`mkFlake` + `perSystem`) or a plain `lib.genAttrs systems` helper |
| 59 | [`prefer-attrvalues-over-mapattrs-identity`](#prefer-attrvalues-over-mapattrs-identity) | err | `lib.mapAttrsToList (_: v: v) X` is `builtins.attrValues X`; drop the identity map |
| 60 | [`prefer-fileset-over-cleansource`](#prefer-fileset-over-cleansource) | err | `lib.cleanSource` is a blunt filter; prefer `lib.fileset.toSource { root; fileset = ...; }` so the source closure names exactly what the build needs |
| 61 | [`prefer-formats-json-generate`](#prefer-formats-json-generate) | err | use `(pkgs.formats.json { }).generate "name" value` instead of `pkgs.writeText "name" (builtins.toJSON value)` |
| 62 | [`prefer-genattrs-listtoattrs`](#prefer-genattrs-listtoattrs) | err | `listToAttrs (map f xs)` is `lib.genAttrs' xs f`; when each entry is keyed by the element itself it simplifies further to `lib.genAttrs xs f` |
| 63 | [`prefer-genattrs-mapattrs-identity`](#prefer-genattrs-mapattrs-identity) | err | `lib.mapAttrs (_: _: v) X` discards both name and value; use `lib.genAttrs (lib.attrNames X) (_: v)` when the value is constant, or `builtins.mapAttrs (_: f) X` when only the value matters |
| 64 | [`prefer-genlist-over-map-range`](#prefer-genlist-over-map-range) | err | `map f (lib.range 0 (n - 1))` collapses to `lib.genList f n` |
| 65 | [`prefer-imap0-over-genlist-identity`](#prefer-imap0-over-genlist-identity) | err | `lib.genList lib.id n` just materializes the index list; iterate the data with `lib.imap0`, or use `lib.range 0 (n - 1)` if you only need the integers |
| 66 | [`prefer-lib-import-format`](#prefer-lib-import-format) | err | use `lib.importJSON path` / `lib.importTOML path` instead of `fromJSON (readFile path)` / `fromTOML (readFile path)` |
| 67 | [`prefer-lib-optional-singleton`](#prefer-lib-optional-singleton) | err | `lib.optionals cond [ x ]` collapses to `lib.optional cond x` |
| 68 | [`prefer-or-default-over-has-attr-guard`](#prefer-or-default-over-has-attr-guard) | err | `(s ? k) && <expr using s.k>` guards a lookup with an existence check; push the default into the lookup with `s.k or DEFAULT` |
| 69 | [`prefer-sorton-over-keyed-sort`](#prefer-sorton-over-keyed-sort) | err | `sort (a: b: (f a) < (f b))` is a keyed comparator; use `lib.sortOn f xs`, which evaluates the key once per element |
| 70 | [`no-recursive-update`](#no-recursive-update) | err | lib.recursiveUpdate silently replaces at leaf collisions; use ix.deepMerge.strict (throws on collision) or ix.deepMerge.rhs (rhs wins) from `lib/util/deep-merge.nix` |
| 71 | [`no-tofile-unsafediscardstringcontext`](#no-tofile-unsafediscardstringcontext) | err | `builtins.toFile X (builtins.unsafeDiscardStringContext Y)` drops the runtime dependency; use `pkgs.writeText X Y` (or `passAsFile`) instead |
| 72 | [`no-handrolled-toml-scalar`](#no-handrolled-toml-scalar) | err | hand-rolled `toToml` scalar encoder; use ix.toml.scalar from `lib/util/toml.nix` (and ix.attrs.flattenToDotted from `lib/util/attrs.nix` for nested config trees) |
| 73 | [`no-at-pattern-shortcut`](#no-at-pattern-shortcut) | err | `{ foo, ... }@args` then reaching `args.bar` hides a required input; match every attribute you use in the formals |
| 74 | [`nixpkgs-explicit-config`](#nixpkgs-explicit-config) | err | `import nixpkgs {}` inherits ambient config and overlays from the environment; pass `config = {}; overlays = [];` |
| 75 | [`import-nixpkgs-once`](#import-nixpkgs-once) | err | an optional `pkgs ? import <nixpkgs> {}` default re-imports Nixpkgs as an accidental singleton; require `pkgs` and thread it through |
| 76 | [`set-docheck`](#set-docheck) | warn | `checkPhase` is off by default; set `doCheck = true;` so the build runs the package's tests |
| 77 | [`declare-env-explicitly`](#declare-env-explicitly) | err | a list attr coerces to a single space-joined env var; use the `env` slot with `lib.escapeShellArgs` for correct conversion |
| 78 | [`extend-makeflagsarray`](#extend-makeflagsarray) | err | assigning `makeFlagsArray` directly mangles space-containing values; append to it in a `preBuild` shell snippet |
| 79 | [`no-pkgs-in-callpackage`](#no-pkgs-in-callpackage) | err | taking `pkgs` in a `callPackage` argument set breaks `override`; list the exact dependencies you need |
| 80 | [`keep-python-composable`](#keep-python-composable) | err | pulling deps out of `python3Packages` blocks per-dependency overrides; take the package names directly |
| 81 | [`future-proof-overrideattrs`](#future-proof-overrideattrs) | err | `overrideAttrs` with an attrset drops pre-existing values; use the `(old: { ... })` function form with `old.x or []` |
| 82 | [`keep-phase-hooks`](#keep-phase-hooks) | err | a phase override without `runHook pre*/post*` strips downstream pre/post hooks; bracket the body with the hook calls |
| 83 | [`prefer-substituteinplace`](#prefer-substituteinplace) | err | `sed -i`/`awk` in a phase fails silently when the match disappears; use `substituteInPlace ... --replace-fail` |
| 84 | [`prefer-phase-flags`](#prefer-phase-flags) | err | a whole-phase override carrying custom targets/flags should be `makeFlags` / `buildFlags` / `configureFlags` / `installTargets` |
| 85 | [`filter-src`](#filter-src) | warn | raw `src = ./.;` copies the whole working tree into the store; filter with `lib.fileset.toSource` |
| 86 | [`pname-with-version`](#pname-with-version) | err | a literal `name = "package"` set alongside `version` restates stdenv; use `pname` |
| 87 | [`cross-compile-ready-deps`](#cross-compile-ready-deps) | err | build-time tools (`pkg-config`, `cmake`, ...) in `buildInputs` break cross-compilation; move them to `nativeBuildInputs` |
| 88 | [`overlay-preserve-nested`](#overlay-preserve-nested) | err | `final: prev: { a = { b; }; }` drops the rest of `prev.a`; merge with `prev.a or {} // { ... }` |
| 89 | [`keep-overrides-composable`](#keep-overrides-composable) | err | hiding a custom package in an overlay `let` blocks later overlays; expose it as a real attr and inject via `final.<name>` |
| 90 | [`parametrize-with-options`](#parametrize-with-options) | err | a top-level function arg on a module locks the choice in; declare an `mkOption` and read it from `config` |
| 91 | [`avoid-specialargs`](#avoid-specialargs) | err | `specialArgs` injection scales badly and can clash; prefer a Nixpkgs overlay and read `pkgs.<name>` |
| 92 | [`separate-host-guest-pkgs`](#separate-host-guest-pkgs) | err | referencing the host's `pkgs` inside a test node breaks when host and guest platforms differ; take `pkgs` from the node module function |
| 93 | [`wait-for-unit-and-port`](#wait-for-unit-and-port) | err | curling a service after only `wait_for_unit` races on fast hosts; wait for `network-online.target`, the unit, and the open port |
| 94 | [`minimize-with-scope`](#minimize-with-scope) | err | `with <expr>;` over any target other than a tightly-scoped `with pkgs;` obscures name origins; bind with `let`/`inherit` |

## Rules by theme

- **Purity & reproducibility** — [`no-builtins-current-system`](#no-builtins-current-system), [`no-builtins-fetch`](#no-builtins-fetch), [`no-builtins-get-env`](#no-builtins-get-env), [`no-impure`](#no-impure), [`no-allow-builtin-fetch-git`](#no-allow-builtin-fetch-git), [`no-builtins-path-without-name`](#no-builtins-path-without-name), [`no-getflake-tostring`](#no-getflake-tostring), [`no-nixpkgs-channel-ref`](#no-nixpkgs-channel-ref), [`no-path-flake-ref`](#no-path-flake-ref), [`no-parent-path`](#no-parent-path), [`no-root-string-interp`](#no-root-string-interp)
- **Hashes & fetchers** — [`no-fake-hash`](#no-fake-hash), [`no-fetchfromgithub-fixed-hash`](#no-fetchfromgithub-fixed-hash), [`prefer-sri-hash`](#prefer-sri-hash)
- **Derivations & stdenv** — [`mkderivation-missing-strict-deps`](#mkderivation-missing-strict-deps), [`no-buildphase-pure-default`](#no-buildphase-pure-default), [`no-buildrustpackage`](#no-buildrustpackage), [`no-derivation-name-version-interp`](#no-derivation-name-version-interp), [`no-fixup-phase-override`](#no-fixup-phase-override), [`no-pname-finalattrs-self-ref`](#no-pname-finalattrs-self-ref), [`no-stringly-flags`](#no-stringly-flags), [`no-substitute-all`](#no-substitute-all), [`runcommand-missing-structured-attrs`](#runcommand-missing-structured-attrs)
- **Generated shell scripts** — [`no-handrolled-shell-script`](#no-handrolled-shell-script), [`no-nix-in-generated-shell`](#no-nix-in-generated-shell), [`no-write-shell-application`](#no-write-shell-application), [`no-write-shell-script`](#no-write-shell-script), [`no-write-shell-script-bin`](#no-write-shell-script-bin)
- **Module system & options** — [`no-mkforce`](#no-mkforce), [`no-mkoverride-numeric`](#no-mkoverride-numeric), [`no-mkoption-conditional-default`](#no-mkoption-conditional-default), [`no-mkif-true`](#no-mkif-true), [`no-mkmerge-singleton`](#no-mkmerge-singleton), [`no-types-attrs`](#no-types-attrs), [`no-types-unspecified`](#no-types-unspecified), [`no-mddoc`](#no-mddoc), [`no-import-in-imports`](#no-import-in-imports)
- **Error handling & diagnostics** — [`no-abort`](#no-abort), [`no-bare-assert`](#no-bare-assert), [`no-throw-without-message`](#no-throw-without-message), [`no-deprecated-trace`](#no-deprecated-trace)
- **Name scoping & qualification** — [`no-with-builtins`](#no-with-builtins), [`no-with-lib`](#no-with-lib), [`no-with-pkgs`](#no-with-pkgs), [`no-pkgs-lib`](#no-pkgs-lib)
- **Redundant, no-op & legacy syntax** — [`no-builtin-length-list-zero`](#no-builtin-length-list-zero), [`no-chained-attrset-update`](#no-chained-attrset-update), [`no-deprecated-iflist-empty`](#no-deprecated-iflist-empty), [`no-double-paren`](#no-double-paren), [`no-empty-list-concat`](#no-empty-list-concat), [`no-negate-bool-literal`](#no-negate-bool-literal), [`no-optional-true`](#no-optional-true), [`no-update-empty-set`](#no-update-empty-set), [`no-unquoted-splice`](#no-unquoted-splice), [`no-legacy-let-block`](#no-legacy-let-block), [`no-rec-attrset`](#no-rec-attrset)
- **Deprecated & discouraged APIs** — [`no-ambiguous-gpl-license`](#no-ambiguous-gpl-license), [`no-flake-utils-eachsystem`](#no-flake-utils-eachsystem)
- **Prefer idiomatic lib / ix helpers** — [`prefer-attrvalues-over-mapattrs-identity`](#prefer-attrvalues-over-mapattrs-identity), [`prefer-fileset-over-cleansource`](#prefer-fileset-over-cleansource), [`prefer-formats-json-generate`](#prefer-formats-json-generate), [`prefer-genattrs-listtoattrs`](#prefer-genattrs-listtoattrs), [`prefer-genattrs-mapattrs-identity`](#prefer-genattrs-mapattrs-identity), [`prefer-genlist-over-map-range`](#prefer-genlist-over-map-range), [`prefer-imap0-over-genlist-identity`](#prefer-imap0-over-genlist-identity), [`prefer-lib-import-format`](#prefer-lib-import-format), [`prefer-lib-optional-singleton`](#prefer-lib-optional-singleton), [`prefer-or-default-over-has-attr-guard`](#prefer-or-default-over-has-attr-guard), [`prefer-sorton-over-keyed-sort`](#prefer-sorton-over-keyed-sort), [`no-recursive-update`](#no-recursive-update), [`no-tofile-unsafediscardstringcontext`](#no-tofile-unsafediscardstringcontext), [`no-handrolled-toml-scalar`](#no-handrolled-toml-scalar)
- **Function arguments & Nixpkgs entry points** — [`no-at-pattern-shortcut`](#no-at-pattern-shortcut), [`nixpkgs-explicit-config`](#nixpkgs-explicit-config), [`import-nixpkgs-once`](#import-nixpkgs-once)
- **Building software with Nixpkgs (Nixcademy)** — [`set-docheck`](#set-docheck), [`declare-env-explicitly`](#declare-env-explicitly), [`extend-makeflagsarray`](#extend-makeflagsarray), [`no-pkgs-in-callpackage`](#no-pkgs-in-callpackage), [`keep-python-composable`](#keep-python-composable), [`future-proof-overrideattrs`](#future-proof-overrideattrs), [`keep-phase-hooks`](#keep-phase-hooks), [`prefer-substituteinplace`](#prefer-substituteinplace), [`prefer-phase-flags`](#prefer-phase-flags), [`filter-src`](#filter-src), [`pname-with-version`](#pname-with-version), [`cross-compile-ready-deps`](#cross-compile-ready-deps)
- **Nixpkgs overlays (Nixcademy)** — [`overlay-preserve-nested`](#overlay-preserve-nested), [`keep-overrides-composable`](#keep-overrides-composable)
- **NixOS modules (Nixcademy)** — [`parametrize-with-options`](#parametrize-with-options), [`avoid-specialargs`](#avoid-specialargs)
- **NixOS tests (Nixcademy)** — [`separate-host-guest-pkgs`](#separate-host-guest-pkgs), [`wait-for-unit-and-port`](#wait-for-unit-and-port)
- **Scoping (Nixcademy)** — [`minimize-with-scope`](#minimize-with-scope)

## Purity & reproducibility

Eval-time impurity, network access during evaluation, and working-tree leakage into the store.

### no-builtins-current-system

**🔴 error**

builtins.currentSystem is impure. Pass system explicitly via flake or function args.

*Matches:* `select_expression` · *predicates:* `text` · *1 pattern variant*

<table><tr><th>flagged</th><th>ok</th></tr><tr><td>

```nix
builtins.currentSystem
```

</td><td>

```nix
{ system }: system
```

</td></tr></table>

### no-builtins-fetch

**🔴 error**

builtins.fetch* runs on eval, is not a fixed-output derivation, and does not substitute. Use the matching pkgs.* fetcher.

*Matches:* `select_expression` · *predicates:* `text-match` · *1 pattern variant*

<table><tr><th>flagged</th><th>ok</th></tr><tr><td>

```nix
builtins.fetchTarball { url = "https://example.com/x.tar.gz"; }
```

</td><td>

```nix
{ pkgs }: pkgs.fetchurl { url = "https://example.com/x.tar.gz"; hash = "sha256-2vBmJBzPyVcEMrAQUegW2BIvBI0Ld38d5fjLnVZQNN0="; }
```

</td></tr></table>

### no-builtins-get-env

**🔴 error**

builtins.getEnv is impure. Pass values explicitly via function args or flake inputs.

*Matches:* `apply_expression` · *predicates:* `text` · *1 pattern variant*

<table><tr><th>flagged</th><th>ok</th></tr><tr><td>

```nix
builtins.getEnv "HOME"
```

</td><td>

```nix
{ home }: home
```

</td></tr></table>

### no-impure

**🔴 error**

__impure = true: prefer ix.writeNushellApplication for side-effect orchestrators. __impure is only for network side effects that must live inside the derivation graph.

*Matches:* `binding` · *predicates:* `text` · *1 pattern variant*

<table><tr><th>flagged</th><th>ok</th></tr><tr><td>

```nix
{ __impure = true; }
```

</td><td>

```nix
{ enabled = true; }
```

</td></tr></table>

### no-allow-builtin-fetch-git

**🔴 error**

`allowBuiltinFetchGit = true` lets `builtins.fetchGit` run while Nix evaluates the package, so ordinary NixOS evaluation can hit the network and block on Git clones. Pin each Cargo git dependency under `rustPlatform.importCargoLock` `outputHashes` instead.

*Matches:* `binding` · *predicates:* `text`, `text-match` · *1 pattern variant*

<table><tr><th>flagged</th><th>ok</th></tr><tr><td>

```nix
{
  cargoLock.lockFile = ./Cargo.lock;
  allowBuiltinFetchGit = true;
}
```

</td><td>

```nix
{
  cargoLock = {
    lockFile = ./Cargo.lock;
    outputHashes = { };
  };
}
```

</td></tr></table>

### no-builtins-path-without-name

**🔴 error**

`builtins.path { path = ...; }` without `name` derives the store-path name from the working directory. Set `name = "<stable>"` so the result is reproducible across clones.

*Matches:* `apply_expression` · *predicates:* `no-descendant`, `text` · *1 pattern variant*

<table><tr><th>flagged</th><th>ok</th></tr><tr><td>

```nix
builtins.path { path = ./src; }
```

</td><td>

```nix
builtins.path { name = "source"; path = ./src; }
```

</td></tr></table>

### no-getflake-tostring

**🔴 error**

`builtins.getFlake (toString ./...)` uses path-flake semantics, copying the entire tree including non-regular files. Use `"git+file:///..."` instead.

*Matches:* `apply_expression` · *predicates:* `text`, `text-match` · *1 pattern variant*

<table><tr><th>flagged</th><th>ok</th></tr><tr><td>

```nix
builtins.getFlake (toString ./.)
```

</td><td>

```nix
builtins.getFlake "git+file:///repo?submodules=1"
```

</td></tr></table>

### no-nixpkgs-channel-ref

**🔴 error**

'<nixpkgs>' channel reference is banned. Use flake inputs.

*Matches:* `spath_expression` · *predicates:* `text` · *1 pattern variant*

<table><tr><th>flagged</th><th>ok</th></tr><tr><td>

```nix
import <nixpkgs> { }
```

</td><td>

```nix
{ nixpkgs }: import nixpkgs { }
```

</td></tr></table>

### no-path-flake-ref

**🔴 error**

Whole-tree `path:` flake refs copy the working tree byte-for-byte. Use `.#...`, `git+file:///abs/path`, a relative `path:./<subtree>` input, or a proper flake input.

*Matches:* `string_expression` · *predicates:* `text-match` · *1 pattern variant*

<table><tr><th>flagged</th><th>ok</th></tr><tr><td>

```nix
{ ref = "path:/home/user/repo"; }
```

</td><td>

```nix
{ ref = "git+file:///home/user/repo"; }
```

</td></tr></table>

### no-parent-path

**🔴 error**

a `../` (parent-directory) path literal reaches across a directory boundary, so moving the file silently breaks the reference. Reach cross-tree through a named handle instead: `ix.<helper>` / `index.lib.<helper>` for code, `ix.paths.<root>` (or `index.lib.paths.<root>`) + a relative string for a file path, or the package registry for a sibling package. Downward `./...` paths are fine. Matches the `..` segment in a path literal only, so comments and strings (e.g. a `"../bad"` traversal-rejection test fixture) are never flagged.

*Matches:* `path_expression` · *predicates:* `text-match` · *1 pattern variant*

<table><tr><th>flagged</th><th>ok</th></tr><tr><td>

```nix
# A `../` path literal reaches out of this file's directory: moving the file
# silently breaks it. This is exactly what the rule forbids.
{ lib }:
import ../util/writers.nix { inherit lib; }
```

</td><td>

```nix
# Cross-tree references go through named handles, not `../`. Downward `./...`
# paths are fine; a root-anchored path uses `ix.paths.<root>` + a relative
# string. A traversal-rejection test fixture string like "../bad" stays a
# string literal and is never a path expression, so the rule ignores it.
{ ix, lib }:
{
  helper = ix.writeBashApplication;
  rules = ix.paths.root + "/astlog-rules/nix.astlog";
  local = import ./sibling.nix { inherit lib; };
  rejected = ix.relativePath.isSafe "../bad";
}
```

</td></tr></table>

### no-root-string-interp

**🔴 error**

`"${root}/..."` string-interpolates the workspace tree into a string, leaking full-tree context. Use `root + "/..."` (path concat) or `builtins.path { name = "..."; path = root + "/..."; }`.

*Matches:* `string_expression` · *predicates:* `text-match` · *1 pattern variant*

<table><tr><th>flagged</th><th>ok</th></tr><tr><td>

```nix
{ root }: "${root}/nix/file.cmake"
```

</td><td>

```nix
{ root }: root + "/nix/file.cmake"
```

</td></tr></table>

## Hashes & fetchers

Source pinning: real SRI hashes, no placeholders, no eval-time GitHub pins.

### no-fake-hash

**🔴 error**

Fake hashes are banned. Compute the real SRI hash before editing tracked Nix files.

*Matches:* `select_expression`, `string_expression` · *predicates:* `text`, `text-match` · *2 pattern variants*

<table><tr><th>flagged</th><th>ok</th></tr><tr><td>

```nix
{ lib }: { hash = lib.fakeHash; }
```

</td><td>

```nix
{ hash = "sha256-2vBmJBzPyVcEMrAQUegW2BIvBI0Ld38d5fjLnVZQNN0="; }
```

</td></tr></table>

### no-fetchfromgithub-fixed-hash

**🔴 error**

Do not pin GitHub source with `fetchFromGitHub { ... hash = ...; }`. Add it as a flake input with `flake = false` and consume that source instead.

*Matches:* `apply_expression` · *predicates:* `text` · *2 pattern variants*

<table><tr><th>flagged</th><th>ok</th></tr><tr><td>

```nix
{ fetchFromGitHub }: fetchFromGitHub { owner = "o"; repo = "r"; rev = "v1.0"; hash = "sha256-2vBmJBzPyVcEMrAQUegW2BIvBI0Ld38d5fjLnVZQNN0="; }
```

</td><td>

```nix
{ sources }: sources.myDep
```

</td></tr></table>

### prefer-sri-hash

**🔴 error**

Legacy `sha256`-flavored hash attr (fetcher `sha256`, or the removed `cargoSha256` / `vendorSha256` / `npmDepsSha256` builder slots). Use the SRI `hash` slot instead. The one legitimate exception is a hex digest that must round-trip verbatim (e.g. a Cargo.lock checksum fed back into `.cargo-checksum.json`): keep it on `sha256` and suppress with an `astlog-ignore: prefer-sri-hash` comment naming the reason.

*Matches:* `binding` · *predicates:* `text-match` · *1 pattern variant*

<table><tr><th>flagged</th><th>ok</th></tr><tr><td>

```nix
{ pkgs }: pkgs.fetchurl { url = "https://example.org/x.tar.gz"; sha256 = "0f5cf6lqbmqghjvqr0xrmkb9cscj0567dyj3cy11d4z2ppy20wmf"; }
```

</td><td>

```nix
{ pkgs }: pkgs.fetchurl { url = "https://example.org/x.tar.gz"; hash = "sha256-2vBmJBzPyVcEMrAQUegW2BIvBI0Ld38d5fjLnVZQNN0="; }
```

</td></tr></table>

## Derivations & stdenv

mkDerivation hygiene: strict deps, no restating stdenv defaults, typed builders.

### mkderivation-missing-strict-deps

**🔴 error**

mkDerivation without strictDeps = true. Add it to prevent build/host dependency leaking.

*Matches:* `apply_expression` · *predicates:* `no-descendant`, `text` · *1 pattern variant*

<table><tr><th>flagged</th><th>ok</th></tr><tr><td>

```nix
{ pkgs }: pkgs.stdenv.mkDerivation { pname = "x"; version = "1"; }
```

</td><td>

```nix
{ pkgs }: pkgs.stdenv.mkDerivation { pname = "x"; version = "1"; strictDeps = true; }
```

</td></tr></table>

### no-buildphase-pure-default

**🔴 error**

`buildPhase = "make";` and `installPhase = "make install";` restate the stdenv default. Drop the override or document the deviation.

*Matches:* `binding` · *predicates:* `text` · *2 pattern variants*

<table><tr><th>flagged</th><th>ok</th></tr><tr><td>

```nix
{ buildPhase = "make"; }
```

</td><td>

```nix
{ postBuild = "make docs"; }
```

</td></tr></table>

### no-buildrustpackage

**🔴 error**

`buildRustPackage` is the legacy entry point. Use `rustPlatform.buildRustPackage` (typed `cargoLock` shape) or `crane`.

*Matches:* `apply_expression` · *predicates:* `text` · *1 pattern variant*

<table><tr><th>flagged</th><th>ok</th></tr><tr><td>

```nix
{ buildRustPackage }: buildRustPackage { pname = "x"; }
```

</td><td>

```nix
{ rustPlatform }: rustPlatform.buildRustPackage { pname = "x"; }
```

</td></tr></table>

### no-derivation-name-version-interp

**🔴 error**

`name = "${pname}-${version}"` restates stdenv. Set `pname` and `version`; stdenv constructs `name` automatically.

*Matches:* `binding` · *predicates:* `text`, `text-match` · *1 pattern variant*

<table><tr><th>flagged</th><th>ok</th></tr><tr><td>

```nix
{ pname, version }: { name = "${pname}-${version}"; inherit pname version; }
```

</td><td>

```nix
{ pname, version }: { inherit pname version; }
```

</td></tr></table>

### no-fixup-phase-override

**🔴 error**

`fixupPhase = ...` wipes the default fixup pass. Use `preFixup` / `postFixup` instead.

*Matches:* `binding` · *predicates:* `text` · *1 pattern variant*

<table><tr><th>flagged</th><th>ok</th></tr><tr><td>

```nix
{ fixupPhase = "true"; }
```

</td><td>

```nix
{ postFixup = "patchShebangs out"; }
```

</td></tr></table>

### no-pname-finalattrs-self-ref

**🔴 error**

`finalAttrs.pname` as the value of `repo`, `owner`, or `pname` itself is usually a copy-paste artifact. Use the literal.

*Matches:* `binding` · *predicates:* `text`, `text-match` · *1 pattern variant*

<table><tr><th>flagged</th><th>ok</th></tr><tr><td>

```nix
finalAttrs: { repo = finalAttrs.pname; }
```

</td><td>

```nix
finalAttrs: { repo = "real-repo-name"; }
```

</td></tr></table>

### no-stringly-flags

**🔴 error**

Build flag attrs are list-of-string, not one string with spaces. Whitespace inside flag values breaks paths-with-spaces and shell quoting.

*Matches:* `binding` · *predicates:* `text-match` · *1 pattern variant*

<table><tr><th>flagged</th><th>ok</th></tr><tr><td>

```nix
{ configureFlags = "--enable-foo --disable-bar"; }
```

</td><td>

```nix
{ configureFlags = [ "--enable-foo" "--disable-bar" ]; }
```

</td></tr></table>

### no-substitute-all

**🔴 error**

`substituteAll` / `substituteAllFiles` was removed from nixpkgs. Use `replaceVars` or `replaceVarsWith` instead.

*Matches:* `select_expression`, `apply_expression` · *predicates:* `text-match` · *2 pattern variants*

<table><tr><th>flagged</th><th>ok</th></tr><tr><td>

```nix
{ pkgs, src, foo }: pkgs.substituteAll { inherit src foo; }
```

</td><td>

```nix
{ pkgs, foo }: pkgs.replaceVars ./template.sh { inherit foo; }
```

</td></tr></table>

### runcommand-missing-structured-attrs

**🟡 warning**

`pkgs.runCommand name {} body` passes an empty attrset, so the derivation gets no `__structuredAttrs = true`. Add `{ __structuredAttrs = true; }` for consistent derivation behavior (attrs passed as JSON: no env-var length limits, preserved list/nested types). Warning severity: a large pre-existing tail still carries `{}`.

*Matches:* `apply_expression` · *predicates:* `text-match` · *1 pattern variant*

<table><tr><th>flagged</th><th>ok</th></tr><tr><td>

```nix
pkgs.runCommand "demo" { } ''
  mkdir -p "$out"
''
```

</td><td>

```nix
pkgs.runCommand "demo" { __structuredAttrs = true; } ''
  mkdir -p "$out"
''
```

</td></tr></table>

## Generated shell scripts

Unchecked / dependency-hiding shell generators; prefer compiled or checked launchers.

### no-handrolled-shell-script

**🟡 warning**

`writeText "*.sh" ...` hand-rolls a shell script with no shellcheck and no declared runtime deps (the same gap that bans writeShellApplication / writeShellScriptBin). Prefer a compiled wrapper: a small Rust launcher built via ix.rustWorkspace (see packages/config-launch, used by packages/agent/codex) for argv0-preserving exec, or ix.writeNushellApplication / ix.writePythonApplication for logic scripts. A launch wrapper that needs install-time `@placeholder@` substitution the writers cannot express may keep writeText with an `astlog-ignore: no-handrolled-shell-script` comment plus a reason.

*Matches:* `apply_expression` · *predicates:* `text-match` · *1 pattern variant*

<table><tr><th>flagged</th><th>ok</th></tr><tr><td>

```nix
{ writeText }: writeText "wrap.sh" "echo hi"
```

</td><td>

```nix
{ ix }: ix.writeNushellApplication { name = "wrap"; text = "print hi"; }
```

</td></tr></table>

### no-nix-in-generated-shell

**🔴 error**

a `writeShellApplication` whose body shells out to `nix run/build/flake check` hides dependencies from the evaluator and cache, repeats evaluation, and moves failures into runtime logs. Pass realized store paths into the script, or move orchestration into the outer workflow. Largely subsumed by `no-write-shell-application` (writeShellApplication is banned outright), but kept as a distinct rule for any suppressed use.

*Matches:* `apply_expression` · *predicates:* `text-match` · *1 pattern variant*

<table><tr><th>flagged</th><th>ok</th></tr><tr><td>

```nix
pkgs.writeShellApplication {
  name = "deploy";
  text = ''
    nix build .#thing
  '';
}
```

</td><td>

```nix
pkgs.writeShellApplication {
  name = "deploy";
  text = ''
    ${thing}/bin/thing run
  '';
}
```

</td></tr></table>

### no-write-shell-application

**🔴 error**

writeShellApplication is banned. Use ix.writeNushellApplication instead.

*Matches:* `identifier` · *predicates:* `text` · *1 pattern variant*

<table><tr><th>flagged</th><th>ok</th></tr><tr><td>

```nix
{ pkgs }: pkgs.writeShellApplication { name = "x"; text = "true"; }
```

</td><td>

```nix
{ ix, pkgs }: ix.writeNushellApplication pkgs { name = "x"; text = "def main [] {}"; }
```

</td></tr></table>

### no-write-shell-script

**🔴 error**

writeShellScript hand-rolls an UNCHECKED shell script (no shellcheck, no declared runtime deps, the same gap that bans writeShellApplication / writeShellScriptBin). Prefer a compiled Rust launcher/tool (ix.rustWorkspace, see packages/config-launch and packages/claude-hooks), or ix.writeNushellApplication / ix.writePythonApplication for logic; for a genuine POSIX-process script that must be bash, use ix.writeBashApplication (checked: bash -n + shellcheck).

*Matches:* `identifier` · *predicates:* `text` · *1 pattern variant*

<table><tr><th>flagged</th><th>ok</th></tr><tr><td>

```nix
{ pkgs }: pkgs.writeShellScript "x" "true"
```

</td><td>

```nix
{ ix, pkgs }: ix.writeBashApplication pkgs { name = "x"; text = "true"; }
```

</td></tr></table>

### no-write-shell-script-bin

**🔴 error**

writeShellScriptBin is banned. Use ix.writeNushellApplication instead.

*Matches:* `identifier` · *predicates:* `text` · *1 pattern variant*

<table><tr><th>flagged</th><th>ok</th></tr><tr><td>

```nix
{ pkgs }: pkgs.writeShellScriptBin "x" "true"
```

</td><td>

```nix
{ ix, pkgs }: ix.writeNushellApplication pkgs { name = "x"; text = "def main [] {}"; }
```

</td></tr></table>

## Module system & options

Option declarations and the priority/merge combinators that compose NixOS config.

### no-mkforce

**🔴 error**

mkForce is banned. Use priority composition (mkDefault, mkOverride) instead.

*Matches:* `identifier` · *predicates:* `text` · *1 pattern variant*

<table><tr><th>flagged</th><th>ok</th></tr><tr><td>

```nix
{ lib }: { services.foo.enable = lib.mkForce true; }
```

</td><td>

```nix
{ lib }: { services.foo.enable = lib.mkDefault true; }
```

</td></tr></table>

### no-mkoverride-numeric

**🔴 error**

`mkOverride <literal>` is the back-door for `mkForce`. Use module priority composition (`mkDefault`, `mkOptionDefault`, `mkBefore`, `mkAfter`) or fix the module boundary.

*Matches:* `apply_expression` · *predicates:* `text-match` · *1 pattern variant*

<table><tr><th>flagged</th><th>ok</th></tr><tr><td>

```nix
{ lib, x }: lib.mkOverride 50 x
```

</td><td>

```nix
{ lib, x }: lib.mkDefault x
```

</td></tr></table>

### no-mkoption-conditional-default

**🔴 error**

`mkOption.default = if cond then ... else ...` couples the declaration to a sibling option. Keep `default` a self-contained literal and move the branch into `config` as `mkIf <cond> (mkDefault ...)`.

*Matches:* `apply_expression` · *predicates:* `text`, `text-match` · *1 pattern variant*

<table><tr><th>flagged</th><th>ok</th></tr><tr><td>

```nix
{ lib, config }: lib.mkOption { default = if config.foo then 1 else 2; }
```

</td><td>

```nix
{ lib }: lib.mkOption { default = 1; }
```

</td></tr></table>

### no-mkif-true

**🔴 error**

`mkIf true x` is `x`; `mkIf false x` is empty. Drop the wrapper.

*Matches:* `apply_expression` · *predicates:* `text-match` · *1 pattern variant*

<table><tr><th>flagged</th><th>ok</th></tr><tr><td>

```nix
{ lib, x }: lib.mkIf true x
```

</td><td>

```nix
{ lib, cond, x }: lib.mkIf cond x
```

</td></tr></table>

### no-mkmerge-singleton

**🔴 error**

`mkMerge [ x ]` is identity. Drop the wrapper.

*Matches:* `apply_expression` · *predicates:* `text-match` · *1 pattern variant*

<table><tr><th>flagged</th><th>ok</th></tr><tr><td>

```nix
{ lib, x }: lib.mkMerge [ x ]
```

</td><td>

```nix
{ lib, x, y }: lib.mkMerge [ x y ]
```

</td></tr></table>

### no-types-attrs

**🔴 error**

`types.attrs` is the give-up type. Use a typed `submodule` with `freeformType` (or `attrsOf <real-type>`) so options merge, document, and validate.

*Matches:* `select_expression` · *predicates:* `text-match` · *1 pattern variant*

<table><tr><th>flagged</th><th>ok</th></tr><tr><td>

```nix
{ lib }: lib.mkOption { type = lib.types.attrs; default = { }; }
```

</td><td>

```nix
{ lib }: lib.mkOption { type = lib.types.attrsOf lib.types.str; default = { }; }
```

</td></tr></table>

### no-types-unspecified

**🔴 error**

`types.unspecified` accepts anything, including bottom. Use a real type or `attrsOf <type>` / `oneOf [ ... ]`.

*Matches:* `select_expression` · *predicates:* `text-match` · *1 pattern variant*

<table><tr><th>flagged</th><th>ok</th></tr><tr><td>

```nix
{ lib }: lib.mkOption { type = lib.types.unspecified; }
```

</td><td>

```nix
{ lib }: lib.mkOption { type = lib.types.str; }
```

</td></tr></table>

### no-mddoc

**🔴 error**

`lib.mdDoc` / `mdDoc` is a no-op and was removed from nixpkgs. Pass the description string directly.

*Matches:* `apply_expression` · *predicates:* `text-match` · *1 pattern variant*

<table><tr><th>flagged</th><th>ok</th></tr><tr><td>

```nix
{ lib }: lib.mdDoc "Module docs."
```

</td><td>

```nix
{ description = "Module docs."; }
```

</td></tr></table>

### no-import-in-imports

**🔴 error**

Use 'imports = [ ./foo.nix ]', not 'imports = [ (import ./foo.nix) ]'. NixOS modules auto-import paths.

*Matches:* `binding` · *predicates:* `text` · *1 pattern variant*

<table><tr><th>flagged</th><th>ok</th></tr><tr><td>

```nix
{ imports = [ (import ./foo.nix) ]; }
```

</td><td>

```nix
{ imports = [ ./foo.nix ]; }
```

</td></tr></table>

## Error handling & diagnostics

Failures and debug output that should name their invariant or not ship at all.

### no-abort

**🔴 error**

`abort` is a hard abort with no recovery. Use `throw` (catchable) or `lib.assertMsg` (typed assertion) instead.

*Matches:* `apply_expression` · *predicates:* `text-match` · *1 pattern variant*

<table><tr><th>flagged</th><th>ok</th></tr><tr><td>

```nix
x: abort "boom"
```

</td><td>

```nix
x: throw "ix.example: boom"
```

</td></tr></table>

### no-bare-assert

**🔴 error**

Bare `assert <cond>;` gives an opaque failure. Use `assert lib.assertMsg <cond> "<why>";` (or `lib.assertOneOf`) so the diagnostic names the invariant.

*Matches:* `assert_expression` · *predicates:* `no-descendant` · *1 pattern variant*

<table><tr><th>flagged</th><th>ok</th></tr><tr><td>

```nix
x: assert x == 1; x
```

</td><td>

```nix
{ lib, x }: assert lib.assertMsg (x == 1) "must be one"; x
```

</td></tr></table>

### no-throw-without-message

**🔴 error**

`throw ""` is an empty placeholder message. Pass a message that names the failing invariant.

*Matches:* `apply_expression` · *predicates:* `text`, `text-match` · *1 pattern variant*

<table><tr><th>flagged</th><th>ok</th></tr><tr><td>

```nix
x: if x then 1 else throw ""
```

</td><td>

```nix
x: if x then 1 else throw "ix.example: x must be set"
```

</td></tr></table>

### no-deprecated-trace

**🔴 error**

`builtins.trace` left in tracked code prints during every eval. Remove it or guard behind a debug flag.

*Matches:* `select_expression` · *predicates:* `text-match` · *1 pattern variant*

<table><tr><th>flagged</th><th>ok</th></tr><tr><td>

```nix
x: builtins.trace "debug" x
```

</td><td>

```nix
x: x
```

</td></tr></table>

## Name scoping & qualification

`with` blocks and pass-through selects that hide where a name comes from.

### no-with-builtins

**🔴 error**

'with builtins;' is banned. Use explicit builtins.foo qualifications.

*Matches:* `with_expression` · *predicates:* `text` · *1 pattern variant*

<table><tr><th>flagged</th><th>ok</th></tr><tr><td>

```nix
with builtins; attrNames { }
```

</td><td>

```nix
builtins.attrNames { }
```

</td></tr></table>

### no-with-lib

**🔴 error**

File-scope 'with lib;' is banned. Use explicit lib.foo qualifications.

*Matches:* `with_expression` · *predicates:* `text` · *1 pattern variant*

<table><tr><th>flagged</th><th>ok</th></tr><tr><td>

```nix
{ lib }: with lib; mkDefault 1
```

</td><td>

```nix
{ lib }: lib.mkDefault 1
```

</td></tr></table>

### no-with-pkgs

**🔴 error**

File-scope 'with pkgs;' is banned. Use explicit pkgs.foo qualifications.

*Matches:* `with_expression` · *predicates:* `text` · *1 pattern variant*

<table><tr><th>flagged</th><th>ok</th></tr><tr><td>

```nix
{ pkgs }: with pkgs; [ hello ]
```

</td><td>

```nix
{ pkgs }: [ pkgs.hello ]
```

</td></tr></table>

### no-pkgs-lib

**🔴 error**

`pkgs.lib.X` reaches through `pkgs`. Use the bare `lib` binding (or `inherit (pkgs) lib;`).

*Matches:* `select_expression` · *predicates:* `text` · *1 pattern variant*

<table><tr><th>flagged</th><th>ok</th></tr><tr><td>

```nix
{ pkgs }: pkgs.lib.attrNames { }
```

</td><td>

```nix
{ lib }: lib.attrNames { }
```

</td></tr></table>

## Redundant, no-op & legacy syntax

Expressions that simplify to something shorter, or banned legacy grammar forms.

### no-builtin-length-list-zero

**🔴 error**

`builtins.length xs == 0` is `xs == [ ]`; same for `> 0` -> `xs != [ ]`. Drop the function call.

*Matches:* `binary_expression` · *predicates:* `text`, `text-match` · *1 pattern variant*

<table><tr><th>flagged</th><th>ok</th></tr><tr><td>

```nix
xs: builtins.length xs == 0
```

</td><td>

```nix
xs: xs == [ ]
```

</td></tr></table>

### no-chained-attrset-update

**🔴 error**

`A // { ... } // { ... }` builds one attrset in two steps. Merge the adjacent literals into a single `// { ... }`. `//` is right-associative, so the chain parses as `A // ({...} // {...})` and the two adjacent literals sit in the right-hand binary expression.

*Matches:* `binary_expression` · *predicates:* — · *1 pattern variant*

<table><tr><th>flagged</th><th>ok</th></tr><tr><td>

```nix
a: a // { x = 1; } // { y = 2; }
```

</td><td>

```nix
a: a // { x = 1; y = 2; }
```

</td></tr></table>

### no-deprecated-iflist-empty

**🔴 error**

`if cond then [ x ] else [ ]` is `lib.optional cond x`.

*Matches:* `if_expression` · *predicates:* `text-match` · *1 pattern variant*

<table><tr><th>flagged</th><th>ok</th></tr><tr><td>

```nix
{ cond, x }: if cond then [ x ] else [ ]
```

</td><td>

```nix
{ lib, cond, x }: lib.optional cond x
```

</td></tr></table>

### no-double-paren

**🔴 error**

`(($X))` is double-grouped. Drop one layer of parens.

*Matches:* `parenthesized_expression` · *predicates:* — · *1 pattern variant*

<table><tr><th>flagged</th><th>ok</th></tr><tr><td>

```nix
f: x: ((f x))
```

</td><td>

```nix
f: x: (f x)
```

</td></tr></table>

### no-empty-list-concat

**🔴 error**

`[] ++ X` and `X ++ []` are no-ops. Drop the empty operand.

*Matches:* `binary_expression` · *predicates:* `text-match` · *2 pattern variants*

<table><tr><th>flagged</th><th>ok</th></tr><tr><td>

```nix
xs: xs ++ [ ]
```

</td><td>

```nix
xs: ys: xs ++ ys
```

</td></tr></table>

### no-negate-bool-literal

**🔴 error**

`!true` / `!false` is the opposite literal. Use the literal directly.

*Matches:* `unary_expression` · *predicates:* `text-match` · *1 pattern variant*

<table><tr><th>flagged</th><th>ok</th></tr><tr><td>

```nix
!true
```

</td><td>

```nix
false
```

</td></tr></table>

### no-optional-true

**🔴 error**

`lib.optional true x` is always `[ x ]`; `lib.optional false x` is always `[ ]`. Inline the literal.

*Matches:* `apply_expression` · *predicates:* `text-match` · *1 pattern variant*

<table><tr><th>flagged</th><th>ok</th></tr><tr><td>

```nix
{ lib, x }: lib.optional true x
```

</td><td>

```nix
{ lib, cond, x }: lib.optional cond x
```

</td></tr></table>

### no-update-empty-set

**🔴 error**

`X // { }` and `{ } // X` are no-ops. Drop the empty operand.

*Matches:* `binary_expression` · *predicates:* `text-match` · *2 pattern variants*

<table><tr><th>flagged</th><th>ok</th></tr><tr><td>

```nix
a: a // { }
```

</td><td>

```nix
{ lib, a, cond }: a // lib.optionalAttrs cond { x = 1; }
```

</td></tr></table>

### no-unquoted-splice

**🔴 error**

`legacyPackages.${system}` interpolates outside a string. Prefer `import nixpkgs { inherit system; }` to get a package set; if you must index dynamically, quote the antiquote: `legacyPackages."${system}"`.

*Matches:* `select_expression` · *predicates:* `text-match` · *1 pattern variant*

<table><tr><th>flagged</th><th>ok</th></tr><tr><td>

```nix
{ nixpkgs, system }: nixpkgs.packages.${system}
```

</td><td>

```nix
{ nixpkgs, system }: import nixpkgs { inherit system; }
```

</td></tr></table>

### no-legacy-let-block

**🔴 error**

`let { ... }` is the undocumented legacy let form. Use `let ... in` or a normal attrset.

*Matches:* `let_attrset_expression` · *predicates:* — · *1 pattern variant*

<table><tr><th>flagged</th><th>ok</th></tr><tr><td>

```nix
let { body = 1; }
```

</td><td>

```nix
let x = 1; in x
```

</td></tr></table>

### no-rec-attrset

**🔴 error**

'rec { }' is banned in derivations and overlays. Use let, finalAttrs:, or final/prev.

*Matches:* `rec_attrset_expression` · *predicates:* — · *1 pattern variant*

<table><tr><th>flagged</th><th>ok</th></tr><tr><td>

```nix
rec { a = 1; b = a; }
```

</td><td>

```nix
let a = 1; in { inherit a; b = a; }
```

</td></tr></table>

## Deprecated & discouraged APIs

Identifiers removed from nixpkgs or superseded by a better pattern.

### no-ambiguous-gpl-license

**🔴 error**

Ambiguous GPL/AGPL/LGPL license identifier. Use the `-Only` / `-Plus` flavor: `gpl2Only`, `gpl3Plus`, `agpl3Only`, etc.

*Matches:* `select_expression` · *predicates:* `text-match` · *1 pattern variant*

<table><tr><th>flagged</th><th>ok</th></tr><tr><td>

```nix
{ lib }: { meta.license = lib.licenses.gpl3; }
```

</td><td>

```nix
{ lib }: { meta.license = lib.licenses.gpl3Plus; }
```

</td></tr></table>

### no-flake-utils-eachsystem

**🔴 error**

`flake-utils.lib.eachSystem` is discouraged. Use `flake-parts` (`mkFlake` + `perSystem`) or a plain `lib.genAttrs systems` helper.

*Matches:* `select_expression` · *predicates:* `text-match` · *1 pattern variant*

<table><tr><th>flagged</th><th>ok</th></tr><tr><td>

```nix
{ flake-utils }: flake-utils.lib.eachDefaultSystem (system: { })
```

</td><td>

```nix
{ lib, systems }: lib.genAttrs systems (system: { })
```

</td></tr></table>

## Prefer idiomatic lib / ix helpers

Hand-rolled combinator chains with a one-call equivalent in `lib` or ix's shared utils.

### prefer-attrvalues-over-mapattrs-identity

**🔴 error**

`lib.mapAttrsToList (_: v: v) X` is `builtins.attrValues X`. Drop the identity map.

*Matches:* `apply_expression` · *predicates:* `same-text`, `text-match` · *1 pattern variant*

<table><tr><th>flagged</th><th>ok</th></tr><tr><td>

```nix
{ lib, attrs }: lib.mapAttrsToList (_: v: v) attrs
```

</td><td>

```nix
{ lib, attrs }: lib.mapAttrsToList (name: v: { inherit name v; }) attrs
```

</td></tr></table>

### prefer-fileset-over-cleansource

**🔴 error**

`lib.cleanSource` is a blunt filter. Prefer `lib.fileset.toSource { root; fileset = ...; }` so the source closure names exactly what the build needs.

*Matches:* `apply_expression` · *predicates:* `text-match` · *1 pattern variant*

<table><tr><th>flagged</th><th>ok</th></tr><tr><td>

```nix
{ lib }: lib.cleanSource ./.
```

</td><td>

```nix
{ lib }: lib.fileset.toSource { root = ./.; fileset = ./src; }
```

</td></tr></table>

### prefer-formats-json-generate

**🔴 error**

Use `(pkgs.formats.json { }).generate "name" value` instead of `pkgs.writeText "name" (builtins.toJSON value)`.

*Matches:* `apply_expression` · *predicates:* `text-match` · *1 pattern variant*

<table><tr><th>flagged</th><th>ok</th></tr><tr><td>

```nix
{ pkgs, value }: pkgs.writeText "data.json" (builtins.toJSON value)
```

</td><td>

```nix
{ pkgs, value }: (pkgs.formats.json { }).generate "data.json" value
```

</td></tr></table>

### prefer-genattrs-listtoattrs

**🔴 error**

`listToAttrs (map f xs)` is `lib.genAttrs' xs f`. When each entry is keyed by the element itself, it simplifies further to `lib.genAttrs xs f`.

*Matches:* `apply_expression` · *predicates:* `text`, `text-match` · *1 pattern variant*

<table><tr><th>flagged</th><th>ok</th></tr><tr><td>

```nix
{ lib, f, xs }: lib.listToAttrs (map f xs)
```

</td><td>

```nix
{ lib, f, xs }: lib.genAttrs' xs f
```

</td></tr></table>

### prefer-genattrs-mapattrs-identity

**🔴 error**

`lib.mapAttrs (_: _: v) X` discards both name and value. Use `lib.genAttrs (lib.attrNames X) (_: v)` when the value is constant, or `builtins.mapAttrs (_: f) X` when only the value matters.

*Matches:* `apply_expression` · *predicates:* `text`, `text-match` · *1 pattern variant*

<table><tr><th>flagged</th><th>ok</th></tr><tr><td>

```nix
{ lib, attrs, v }: lib.mapAttrs (_: _: v) attrs
```

</td><td>

```nix
{ lib, attrs, f }: lib.mapAttrs (_: f) attrs
```

</td></tr></table>

### prefer-genlist-over-map-range

**🔴 error**

`map f (lib.range 0 (n - 1))` collapses to `lib.genList f n`.

*Matches:* `apply_expression` · *predicates:* `text`, `text-match` · *1 pattern variant*

<table><tr><th>flagged</th><th>ok</th></tr><tr><td>

```nix
{ lib, f, count }: map f (lib.range 0 (count - 1))
```

</td><td>

```nix
{ lib, f, count }: lib.genList f count
```

</td></tr></table>

### prefer-imap0-over-genlist-identity

**🔴 error**

`lib.genList lib.id n` just materializes the index list `[ 0 1 ... n-1 ]`. Iterate the data with `lib.imap0`, or use `lib.range 0 (n - 1)` if you only need the integers.

*Matches:* `apply_expression` · *predicates:* `same-text`, `text-match` · *2 pattern variants*

<table><tr><th>flagged</th><th>ok</th></tr><tr><td>

```nix
{ lib, count }: lib.genList lib.id count
```

</td><td>

```nix
{ lib, f, xs }: lib.imap0 (i: x: f i x) xs
```

</td></tr></table>

### prefer-lib-import-format

**🔴 error**

Use `lib.importJSON path` / `lib.importTOML path` instead of `fromJSON (readFile path)` / `fromTOML (readFile path)`.

*Matches:* `apply_expression` · *predicates:* `text-match` · *1 pattern variant*

<table><tr><th>flagged</th><th>ok</th></tr><tr><td>

```nix
path: builtins.fromJSON (builtins.readFile path)
```

</td><td>

```nix
{ lib, path }: lib.importJSON path
```

</td></tr></table>

### prefer-lib-optional-singleton

**🔴 error**

`lib.optionals cond [ x ]` collapses to `lib.optional cond x`.

*Matches:* `apply_expression` · *predicates:* `text-match` · *1 pattern variant*

<table><tr><th>flagged</th><th>ok</th></tr><tr><td>

```nix
{ lib, cond, x }: lib.optionals cond [ x ]
```

</td><td>

```nix
{ lib, cond, x }: lib.optional cond x
```

</td></tr></table>

### prefer-or-default-over-has-attr-guard

**🔴 error**

`(s ? k) && <expr using s.k>` guards a lookup with an existence check. Push the default into the lookup with `s.k or DEFAULT`.

*Matches:* `binary_expression`, `if_expression` · *predicates:* `same-text` · *4 pattern variants*

<table><tr><th>flagged</th><th>ok</th></tr><tr><td>

```nix
s: (s ? enable) && s.enable
```

</td><td>

```nix
s: s.enable or false
```

</td></tr></table>

### prefer-sorton-over-keyed-sort

**🔴 error**

`sort (a: b: (f a) < (f b))` is a keyed comparator. Use `lib.sortOn f xs`, which evaluates the key once per element.

*Matches:* `apply_expression` · *predicates:* `same-text`, `text-match` · *3 pattern variants*

<table><tr><th>flagged</th><th>ok</th></tr><tr><td>

```nix
{ lib, xs }: lib.sort (a: b: a.priority < b.priority) xs
```

</td><td>

```nix
{ lib, xs }: lib.sortOn (x: x.priority) xs
```

</td></tr></table>

### no-recursive-update

**🔴 error**

lib.recursiveUpdate silently replaces at leaf collisions. Use ix.deepMerge.strict (throws on collision) or ix.deepMerge.rhs (rhs wins) from `lib/util/deep-merge.nix`.

*Matches:* `apply_expression` · *predicates:* `text-match` · *1 pattern variant*

<table><tr><th>flagged</th><th>ok</th></tr><tr><td>

```nix
{ lib, a, b }: lib.recursiveUpdate a b
```

</td><td>

```nix
{ ix, a, b }: ix.deepMerge.strict a b
```

</td></tr></table>

### no-tofile-unsafediscardstringcontext

**🔴 error**

`builtins.toFile X (builtins.unsafeDiscardStringContext Y)` drops the runtime dependency. Use `pkgs.writeText X Y` (or `passAsFile`) instead.

*Matches:* `apply_expression` · *predicates:* `text-match` · *1 pattern variant*

<table><tr><th>flagged</th><th>ok</th></tr><tr><td>

```nix
v: builtins.toFile "out.json" (builtins.unsafeDiscardStringContext v)
```

</td><td>

```nix
{ pkgs, v }: pkgs.writeText "out.json" v
```

</td></tr></table>

### no-handrolled-toml-scalar

**🔴 error**

a local `toToml` binding re-rolls the scalar -> TOML literal encoder used to render `key = value` config flags. Use ix.toml.scalar from `lib/util/toml.nix` (and ix.attrs.flattenToDotted from `lib/util/attrs.nix` to collapse a nested config tree into dotted-leaf keys). Matches the binding name only; `toToml` exists nowhere else, so it is the tell-tale of a copy-pasted encoder.

*Matches:* `binding` · *predicates:* `text` · *1 pattern variant*

<table><tr><th>flagged</th><th>ok</th></tr><tr><td>

```nix
let toToml = v: toString v; in toToml 1
```

</td><td>

```nix
{ ix }: ix.toml.scalar 1
```

</td></tr></table>

## Function arguments & Nixpkgs entry points

Surface required inputs in the formals and import Nixpkgs once, explicitly configured.

### no-at-pattern-shortcut

**🔴 error**

Capturing the whole argument set with `{ foo, ... }@args` and then reading `args.bar` hides that `bar` is a required input: readers scanning the formals see only `foo`. Match every attribute you actually use. The `@` capture is fine when you genuinely forward the whole set onward (no `.` access), so the rule fires only when the at-bound name is used as a select base inside the body.

*Matches:* `function_expression` · *predicates:* `same-text`, `ancestor` · *1 pattern variant*

<table><tr><th>flagged</th><th>ok</th></tr><tr><td>

```nix
{ foo, ... }@inputs: foo + inputs.bar
```

</td><td>

```nix
{ foo, bar, ... }: foo + bar
```

</td></tr></table>

### nixpkgs-explicit-config

**🔴 error**

`import nixpkgs {}` starts from a default configuration that can pick up values from an environment variable or the user's home directory. Pass `config = {}` and `overlays = []` so the package set is self-contained.

*Matches:* `apply_expression` · *predicates:* `no-descendant`, `text` · *1 pattern variant*

<table><tr><th>flagged</th><th>ok</th></tr><tr><td>

```nix
{ nixpkgs }: import nixpkgs { }
```

</td><td>

```nix
{ nixpkgs }: import nixpkgs { config = { }; overlays = [ ]; }
```

</td></tr></table>

### import-nixpkgs-once

**🔴 error**

A convenience default like `pkgs ? import <nixpkgs> {}` lets a caller forget to pass the `pkgs` it already has, so the file imports Nixpkgs *again* with a possibly different release, config, and overlays. Drop the optional default, require `pkgs`, and thread the one instance through.

*Matches:* `formal` · *predicates:* `text-match` · *1 pattern variant*

<table><tr><th>flagged</th><th>ok</th></tr><tr><td>

```nix
{ pkgs ? import <nixpkgs> { } }: { extraPkgs = pkgs.hello; }
```

</td><td>

```nix
{ pkgs }: { extraPkgs = pkgs.hello; }
```

</td></tr></table>

## Building software with Nixpkgs (Nixcademy)

mkDerivation hygiene: filtered sources, typed flags, composable overrides, intact phases.

### set-docheck

**🟡 warning**

`stdenv.mkDerivation` can run a package's unit tests in `checkPhase`, but for historical reasons that phase is disabled by default. Set `doCheck = true;` in as many packages as you can. Some wrappers (`buildPythonPackage`) already default it on. Warning severity: it is advice, not a behavior bug.

*Matches:* `apply_expression` · *predicates:* `no-descendant`, `text-match` · *1 pattern variant*

<table><tr><th>flagged</th><th>ok</th></tr><tr><td>

```nix
{ stdenv }: stdenv.mkDerivation { pname = "x"; version = "1"; }
```

</td><td>

```nix
{ stdenv }: stdenv.mkDerivation { pname = "x"; version = "1"; doCheck = true; }
```

</td></tr></table>

### declare-env-explicitly

**🔴 error**

Attributes in the `mkDerivation` argument set become environment variables, with values implicitly coerced to strings. A list coerces to one space-joined string, so `[ "-a" "-b c d" ]` becomes `"-a -b c d"`. Use the `env` slot and convert explicitly with `lib.escapeShellArgs`. Heuristic: an `UPPER_SNAKE` attr name bound to a list.

*Matches:* `binding` · *predicates:* `text-match` · *1 pattern variant*

<table><tr><th>flagged</th><th>ok</th></tr><tr><td>

```nix
{ SOME_ENV_VAR = [ "-a" "-b c d" ]; }
```

</td><td>

```nix
{ lib }: { env.SOME_ENV_VAR = lib.escapeShellArgs [ "-a" "-b c d" ]; }
```

</td></tr></table>

### extend-makeflagsarray

**🔴 error**

Setting `makeFlagsArray` directly re-splits values containing spaces, so `[ "CFLAGS=-O0 -g" ... ]` reaches `make` mangled. Extend the existing array in a `preBuild` shell snippet with proper bash quoting so each flag stays intact.

*Matches:* `binding` · *predicates:* `text` · *1 pattern variant*

<table><tr><th>flagged</th><th>ok</th></tr><tr><td>

```nix
{ makeFlagsArray = [ "CFLAGS=-O0 -g" "LDFLAGS=-la -lb" ]; }
```

</td><td>

```nix
{ preBuild = ''
  makeFlagsArray+=("CFLAGS=-O0 -g" "LDFLAGS=-la -lb")
''; }
```

</td></tr></table>

### no-pkgs-in-callpackage

**🔴 error**

`callPackage` fills named parameters from `pkgs` and adds an `override` attribute for re-evaluating with modified arguments. Taking `pkgs` itself routes dependencies through `pkgs.foo`, which `override` cannot reach. List the exact dependencies as parameters so each is overridable. Scoped to package functions: a `pkgs` formal on a function whose body builds a derivation (`mkDerivation` / `buildPythonPackage` / `buildGoModule` / ...), so a NixOS module or flake output that legitimately takes `pkgs` is not flagged.

*Matches:* `formal` · *predicates:* `text`, `text-match`, `ancestor` · *1 pattern variant*

<table><tr><th>flagged</th><th>ok</th></tr><tr><td>

```nix
{ pkgs, stdenv }: stdenv.mkDerivation { buildInputs = [ pkgs.foo pkgs.bar ]; }
```

</td><td>

```nix
{ stdenv, foo, bar }: stdenv.mkDerivation { buildInputs = [ foo bar ]; }
```

</td></tr></table>

### keep-python-composable

**🔴 error**

`buildPythonPackage` is built on `stdenv.mkDerivation`; like `callPackage` / `override`, pulling dependencies out of `python3Packages` makes overriding a specific Python dependency much harder. Take the package names directly: each interpreter's `callPackage` adds its packages to scope.

*Matches:* `select_expression` · *predicates:* `text-match` · *1 pattern variant*

<table><tr><th>flagged</th><th>ok</th></tr><tr><td>

```nix
{ buildPythonPackage, python3Packages }: buildPythonPackage { propagatedBuildInputs = [ python3Packages.numpy ]; }
```

</td><td>

```nix
{ buildPythonPackage, numpy }: buildPythonPackage { propagatedBuildInputs = [ numpy ]; }
```

</td></tr></table>

### future-proof-overrideattrs

**🔴 error**

`overrideAttrs` with a plain attribute set is destructive: if the original package later adds, say, patches, this override silently drops them. Pass the override *function* form `(old: { ... })` and reference the prior value defensively with `old.patches or []` so you extend instead of replace.

*Matches:* `apply_expression` · *predicates:* `text-match` · *1 pattern variant*

<table><tr><th>flagged</th><th>ok</th></tr><tr><td>

```nix
package.overrideAttrs { patches = [ ./my-bugfix.patch ]; }
```

</td><td>

```nix
package.overrideAttrs (old: { patches = old.patches or [ ] ++ [ ./my-bugfix.patch ]; })
```

</td></tr></table>

### keep-phase-hooks

**🔴 error**

When you replace a phase wholesale, keep **both** the `runHook pre<Phase>` and `runHook post<Phase>` calls for that exact phase. Without them, downstream consumers can no longer prepend or append actions via `preX` / `postX` overrides. The rule negates a per-phase helper that holds only when both the matching pre and post hook are present, so it fires on any phase string missing either hook (or carrying a different phase's hooks), not just one with no hook at all.

*Matches:* `binding` · *predicates:* `text-match`, `not` · *1 pattern variant*

<table><tr><th>flagged</th><th>ok</th></tr><tr><td>

```nix
{ buildPhase = ''
  runHook preBuild
  make something
  do something else
''; }
```

</td><td>

```nix
{ buildPhase = ''
  runHook preBuild
  make something
  do something else
  runHook postBuild
''; }
```

</td></tr></table>

### prefer-substituteinplace

**🔴 error**

`sed` / `awk` in a patch phase do the substitution but do not complain when the pattern stops matching after a typo or an upstream change, so the edit silently becomes a no-op. The builtin `substituteInPlace ... --replace-fail` is easier to read and fails loudly when its match disappears.

*Matches:* `binding` · *predicates:* `text-match` · *1 pattern variant*

<table><tr><th>flagged</th><th>ok</th></tr><tr><td>

```nix
{ postPatch = "sed -i 's/foo/bar/g' main.c"; }
```

</td><td>

```nix
{ postPatch = "substituteInPlace main.c --replace-fail foo bar"; }
```

</td></tr></table>

### prefer-phase-flags

**🔴 error**

Overriding an entire phase to get one specific behavior also disables everything else that phase does, and a later override cannot then add to it. Use the dedicated parameters instead: `makeFlags` / `buildFlags`, `configureFlags`, `installTargets`. They read better and compose. Distinct from `no-buildphase-pure-default` (which only catches a phase restating the stdenv default).

*Matches:* `binding` · *predicates:* `text-match` · *2 pattern variants*

<table><tr><th>flagged</th><th>ok</th></tr><tr><td>

```nix
{ buildPhase = "make target1 target2 FOO=123"; }
```

</td><td>

```nix
{ makeFlags = [ "FOO=123" ]; buildFlags = [ "target1" "target2" ]; }
```

</td></tr></table>

### filter-src

**🟡 warning**

`src = ./.;` copies *everything* in the directory into the Nix store, so any unrelated change triggers a pointless rebuild and bloats the store. Filter with `lib.fileset.toSource` to name exactly what the build needs (`lib.cleanSource ./.` is an acceptable middle ground). Warning severity: a large pre-existing tree can adopt this incrementally.

*Matches:* `binding` · *predicates:* `text`, `text-match` · *1 pattern variant*

<table><tr><th>flagged</th><th>ok</th></tr><tr><td>

```nix
{ stdenv }: stdenv.mkDerivation { src = ./.; }
```

</td><td>

```nix
{ lib }: lib.fileset.toSource { root = ./.; fileset = lib.fileset.gitTracked ./.; }
```

</td></tr></table>

### pname-with-version

**🔴 error**

If a package already defines `version`, use `pname` instead of a literal `name`; `mkDerivation` concatenates them into `<pname>-<version>`. Setting both a literal `name` and `version` restates what stdenv derives. Joined on the shared binding set so it fires only when both siblings are present, and the literal (non-interpolated) name keeps it distinct from `no-derivation-name-version-interp`.

*Matches:* `binding` · *predicates:* `text`, `text-match` · *1 pattern variant*

<table><tr><th>flagged</th><th>ok</th></tr><tr><td>

```nix
{ name = "package"; version = "1.0"; }
```

</td><td>

```nix
{ pname = "package"; version = "1.0"; }
```

</td></tr></table>

### cross-compile-ready-deps

**🔴 error**

Putting compile-time tools (`pkg-config`, `cmake`, `meson`, `ninja`) in `buildInputs` mixes them with run-time libraries and breaks cross-compilation. Move build-host tools to `nativeBuildInputs` and set `strictDeps = true` so the separation is enforced.

*Matches:* `binding` · *predicates:* `text`, `text-match` · *1 pattern variant*

<table><tr><th>flagged</th><th>ok</th></tr><tr><td>

```nix
{ stdenv, boost, pkg-config }: stdenv.mkDerivation { buildInputs = [ boost pkg-config ]; }
```

</td><td>

```nix
{ stdenv, boost, pkg-config }: stdenv.mkDerivation { nativeBuildInputs = [ pkg-config ]; buildInputs = [ boost ]; strictDeps = true; }
```

</td></tr></table>

## Nixpkgs overlays (Nixcademy)

Overlay functions that preserve and re-expose what they extend.

### overlay-preserve-nested

**🔴 error**

An overlay that writes `a = { b = foo; };` at the top level of the returned set replaces the whole `pkgs.a`, dropping everything else that lived inside it. Reference the existing set through `prev` and merge with `//`, guarding with `or {}`. Scoped to **direct members** of the overlay body (via `parent`), so an attrset-valued binding nested deeper (a `meta`, an `overrideAttrs` lambda) is not flagged.

*Matches:* `binding` · *predicates:* `text`, `parent` · *1 pattern variant*

<table><tr><th>flagged</th><th>ok</th></tr><tr><td>

```nix
final: prev: { a = { b = foo; }; }
```

</td><td>

```nix
final: prev: { a = prev.a or { } // { b = foo; }; }
```

</td></tr></table>

### keep-overrides-composable

**🔴 error**

Building a custom package inside an overlay's `let` (then injecting it) hides it, so a later overlay can no longer override it before injection. Expose it as a real attribute and inject it via `final.<name>` so downstream overlays can still `overrideAttrs` it.

*Matches:* `let_expression` · *predicates:* `text-match`, `ancestor` · *1 pattern variant*

<table><tr><th>flagged</th><th>ok</th></tr><tr><td>

```nix
final: prev: let myBoost = prev.callPackage ./boost { }; in { mumble = prev.callPackage ./mumble { boost = myBoost; }; }
```

</td><td>

```nix
final: prev: { myBoost = prev.callPackage ./boost { }; mumble = prev.callPackage ./mumble { boost = final.myBoost; }; }
```

</td></tr></table>

## NixOS modules (Nixcademy)

Parametrize modules through options and overlays, not function arguments or specialArgs.

### parametrize-with-options

**🔴 error**

Wrapping a module in a top-level function argument (`myPackage: { config, lib, ... }: ...`) lets the import site pick the package, but once injected the choice can no longer be changed. Declare a `services.foo.package` option and read `config.services.foo.package` instead.

*Matches:* `function_expression` · *predicates:* `text-match` · *1 pattern variant*

<table><tr><th>flagged</th><th>ok</th></tr><tr><td>

```nix
myPackage: { config, lib, ... }: { config.systemd.services.foo.serviceConfig.ExecStart = "${myPackage}/bin/cmd"; }
```

</td><td>

```nix
{ config, lib, ... }: { options.services.foo.package = lib.mkOption { type = lib.types.package; }; config.systemd.services.foo.serviceConfig.ExecStart = "${config.services.foo.package}/bin/cmd"; }
```

</td></tr></table>

### avoid-specialargs

**🔴 error**

`specialArgs` injects values straight into every module's function header, which scales badly: many modules each wanting a list of `specialArgs`, with the risk of clashes. Prefer a Nixpkgs overlay that defines the package and read it as `pkgs.<name>`.

*Matches:* `binding` · *predicates:* `text` · *1 pattern variant*

<table><tr><th>flagged</th><th>ok</th></tr><tr><td>

```nix
nixpkgs.lib.nixosSystem { specialArgs = { myPkg = self.packages.x86_64-linux.foo; }; modules = [ ./configuration.nix ]; }
```

</td><td>

```nix
nixpkgs.lib.nixosSystem { modules = [ { nixpkgs.overlays = [ self.overlays.default ]; } ./configuration.nix ]; }
```

</td></tr></table>

## NixOS tests (Nixcademy)

Integration tests that stay portable and deterministic on fast and slow hosts.

### separate-host-guest-pkgs

**🔴 error**

Referencing the orchestrating host's `pkgs` instance inside a test node pulls guest packages from the wrong set. It often works, but breaks when host and guest differ (for example a macOS host). Take `pkgs` from each node's module function. Fires on a node defined as a bare attrset that references `pkgs` under a `runNixOSTest` call.

*Matches:* `apply_expression` · *predicates:* `text-match`, `ancestor` · *1 pattern variant*

<table><tr><th>flagged</th><th>ok</th></tr><tr><td>

```nix
{ pkgs }: pkgs.testers.runNixOSTest { nodes.machine = { environment.systemPackages = [ pkgs.hello ]; }; }
```

</td><td>

```nix
{ pkgs }: pkgs.testers.runNixOSTest { nodes.machine = { pkgs, ... }: { environment.systemPackages = [ pkgs.hello ]; }; }
```

</td></tr></table>

### wait-for-unit-and-port

**🔴 error**

A test that curls a service right after `wait_for_unit` flakes on a fast multi-core CI box: packets reach the service while it is still starting. Wait for `network-online.target`, then the unit, then the open port. Fires on a test script that has `wait_for_unit` + `curl` but no `wait_for_open_port`.

*Matches:* `indented_string_expression` · *predicates:* `text-match`, `not` · *1 pattern variant*

<table><tr><th>flagged</th><th>ok</th></tr><tr><td>

```nix
testScript = ''
  server.wait_for_unit("httpd.service")
  client.succeed("curl http://server")
'';
```

</td><td>

```nix
testScript = ''
  server.wait_for_unit("network-online.target")
  server.wait_for_unit("httpd.service")
  server.wait_for_open_port(80)
  client.succeed("curl http://server")
'';
```

</td></tr></table>

## Scoping (Nixcademy)

Keep name origins explicit instead of lifting everything into scope.

### minimize-with-scope

**🔴 error**

`with x;` lifts every key of `x` into scope, so a reader cannot tell whether `hello` is local or comes from `x`. Generalizes the named `no-with-{builtins,lib,pkgs}` bans to `with` over any other target (`with import nixpkgs {};`, `with someAttrset;`, nested `with s; with t;`). Bind names with `let` / `inherit`, or pull packages out with `builtins.attrValues { inherit (pkgs) ...; }`.

*Matches:* `with_expression` · *predicates:* `text-match`, `not` · *1 pattern variant*

<table><tr><th>flagged</th><th>ok</th></tr><tr><td>

```nix
with import nixpkgs { }; with lib; [ (getLib hello) ]
```

</td><td>

```nix
{ pkgs }: let inherit (pkgs) lib; in [ (lib.getLib pkgs.hello) ]
```

</td></tr></table>
