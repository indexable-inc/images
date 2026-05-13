# ix/images

## Workflow

Commit and push after making changes by default.

Contributor setup and local checks are in @CONTRIBUTING.md.

For PR-sized changes, work in a dedicated git worktree instead of the shared checkout. Keep the main checkout on `main` so other sessions and tools see a stable tree. Put active worktrees outside the repo, for example `/tmp/$USER/index-worktrees/<topic>`, and run repo commands from that worktree. If using natural-language code search, run `mgrep search -c {query}` from the main non-worktree checkout, because `mgrep` can be slow or stale inside worktrees.

When a commit actually fixes a tracked GitHub issue, include an auto-closing keyword in the commit body, for example `Fixes #123`, `Closes #123`, or `Resolves #123`. Use `Refs #123` only for related work, policy docs, investigation, or partial cleanup that should not close the issue.

## Debugging VMs

Use the real ix CLI to inspect running VMs before inferring from source. Prefer machine-readable host commands when available, for example `ix ls --output json`.

Run guest commands with `ix shell <vm> -- <cmd> ...`. If command lookup behaves differently from an interactive shell, use absolute paths from the guest, for example `ix shell minecraft -- /run/current-system/sw/bin/journalctl --no-pager -u minecraft.service -n 80` or `ix shell minecraft -- /bin/sh -lc 'ps -ef'`.

When a debugging tool is not installed on the host or in the dev shell, run it through nixpkgs instead of hand-installing it, for example `nix run nixpkgs#jq -- --version` or `nix run nixpkgs#curl -- --version`. Put arguments after `--`.

For service failures, check the rendered unit and the live journal inside the VM. Confirm whether the unit exists, whether PID 1 is systemd, and whether the process is failing after launch before changing image/module code.

## Overview

Pre-built OCI images for ix VMs, plus composable NixOS modules. All images target AMD EPYC Gen 5 (Turin, Zen 5). The base layer sets `nixpkgs.hostPlatform.gcc.arch = "znver5"` so every package in the closure is compiled with `-march=znver5 -mtune=znver5`. No binary cache hits: everything builds from source.

## How it works

Every image is an independent NixOS system closure: `boot.isContainer = true`, systemd as PID 1, no kernel, no bootloader. `lib.mkImage` runs `nixpkgs.lib.nixosSystem` over the platform config (`lib/ix-platform.nix`), OCI packaging (`lib/ix-oci-layer.nix`), the module registry (`modules/`), and any caller modules, then packages the toplevel into an OCI archive via `dockerTools.streamLayeredImage` plus a small docker-archive-to-OCI converter (`lib/docker-to-oci.py`).

Images are not stacked at runtime. ix runs one image. Layering is purely a build-time concern: the closure is split into ~67 OCI layers so the registry stores each shared store path once and clients only pay for deltas. Single-layer would force every image to ship a private copy of the multi-hundred-MB base closure.

## VM assumptions

ix VMs implicitly have snapshots and effectively unbounded disk. Fleet and stateful-service designs should lean on those primitives: take snapshots before destructive or data-format-changing operations, prefer in-place NixOS/system switches for stateful nodes, and do not design around fixed-root-disk exhaustion as a primary constraint.

## Registry access

Do not assume every `registry.ix.dev` image is public. The `ix` namespace is system-owned, so shared bootstrap refs such as `registry.ix.dev/ix/test-cluster-bootstrap:<tag>` are expected to be public. User images live under `registry.ix.dev/<username>/<image>:<tag>` and default to private; private images require the owner’s auth and should behave like not-found for other users. When debugging image pulls, distinguish a public system bootstrap image from a user-owned private image before treating access as a registry outage.

The shared fleet bootstrap image is defined at `images/system/test-cluster-bootstrap`. It is an ordinary image that extends the repo base profile; keep source-switch tools such as `gnutar`, `zstd`, and `gzip` in `modules/profiles/base.nix` so any VM that has been switched once can be switched again. Build the bootstrap with `nix build .#test-cluster-bootstrap --print-out-paths --no-link`; upload it only with an admin ix profile, using an explicit system namespace ref such as `ix image push <archive>.tar registry.ix.dev/ix/test-cluster-bootstrap:<tag>`. TODO: replace full payload uploads with CAS/CDC-aware transfer for both bootstrap image publishing and `ix switch --source`, so routine updates send only changed chunks and then update the registry or switch input reference.

## Layout

```
flake.nix                                  # manifest: inputs + delegated outputs
.envrc, .githooks/pre-commit               # direnv sets core.hooksPath -> nix run .#lint
lib/
  default.nix                              # mkImage, discoverImages, ix.artifacts, helpers
  per-system.nix                           # per-system packages / apps / checks / formatter
  ix-platform.nix                          # target platform: EPYC Gen 5 (znver5), container mode
  ix-oci-layer.nix                         # OCI packaging, base profile
  minecraft-loader.nix                     # helper used by loader modules
  docker-to-oci.py                         # docker-archive -> OCI archive transcoder
modules/
  default.nix                              # canonical module registry (attrset)
  profiles/base.nix                        # CLI tools, on by default
  services/<name>.nix                      # opt-in service
  services/<family>/{default,...}.nix      # service family (runtime + plugins)
images/
  <category>/<name>/default.nix            # NixOS module
  <category>/<name>/versions.nix           # optional: per-version overlay modules
template/                                  # `nix flake init` starter
nix/rules/                                 # ast-grep lint rules
```

## Flake.nix style

`flake.nix` is the repo's handle. It should read like a manifest: a small inputs block and an `outputs` body that is mostly delegation. All logic lives in `./lib/` or behind discovery (`ix.discoverImages`). The goal is that someone landing on `flake.nix` cold can answer "what does this flake expose?" by skimming, not by parsing.

Do not put inside `flake.nix`:

- Fetched-artifact URLs. They are data, not flake-graph participants; keep URL + SRI hash beside the catalog entry and call `pkgs.fetchurl` at use.
- App wrapper definitions (`writeNushellApplication { ... }` for `lint`, `update-mods`, `ix-fleet`, demo wrappers, etc.). Define them in a dedicated module under `./lib/` and reference them from `outputs` by name.
- Per-system `let`-bindings that compose many helpers. Push the composition into a single `mkOutputs system` function in `./lib/` and call it from `lib.genAttrs devSystems`.
- Example or demo wiring (`claudeCodeDemoFor`, per-VM wrappers, etc.). Move into the example's own `default.nix` and import it once.

Target: `flake.nix` fits comfortably in a single screen and its body would look almost unsurprising as JSON. The cost of an inline helper today is the year-from-now untangle. Pay the structure cost up front.

Flake outputs stay on the standard schema: `packages`, `apps`, `checks`, `formatter`, `devShells`, `templates`, `overlays`, `nixosModules`, `lib`. Use `nixosModules` (plural, namespaced) for module exports. Do not add a flat top-level `modules` key: it is non-standard, not validated by `nix flake check`, and may not be discovered by downstream tooling.

## Adding an image

Drop a NixOS module at `images/<category>/<name>/default.nix`. That's it: discovery picks it up on the next eval and exposes `packages.<host>.<name>` for the supported dev systems. The derivation still targets `x86_64-linux`. No flake edits, no registry edits.

For a versioned image (multiple variants ship at once), add a `versions.nix` sibling:

```nix
{ lib, ... }:
let
  default = "26w17a-fabric";
  variants = {
    "26w17a-fabric" = {
      loader = "fabric";
      /* loader-specific args */
    };
  };
in
{
  inherit default;
}
// lib.mapAttrs (tag: { loader, ... }@cfg: {
  ix.image.tag = tag;
  services.minecraft.${loader} = (builtins.removeAttrs cfg [ "loader" ]) // {
    enable = true;
  };
}) variants
```

Discovery then exposes `<name>_<ver>` for each version key plus `<name>` as an alias for the `default` version.

## Adding a module

Drop the file at `modules/services/<name>.nix` (or `modules/profiles/<name>.nix`) and register it in `modules/default.nix`. Keep modules independent: declare `options`, gate everything behind `mkIf cfg.enable`, never import another module. The registry exists so option sets are visible to every image; modules stay inert until their `enable` flag is set.

## Service families

When several modules vary along one axis (e.g. minecraft + fabric/paper/vanilla loaders), put the runtime in `modules/services/<name>/default.nix` and each variant in `modules/services/<name>/<variant>.nix`. The runtime declares a "slot" option; variants fill it. Wire each file into `modules/default.nix` as a separate registry entry.

For minecraft this means:
- `services.minecraft.serverJar` is the slot, declared by the runtime.
- Each loader module (fabric, folia, neoforge, paper, purpur, spigot, sponge, vanilla) sets `services.minecraft.serverJar` from its own URL/version options.
- Enabling a loader auto-enables the runtime via `mkDefault`.
- Enabling two loaders is a module-merge conflict → loud eval error.

## Mods

All mods go in `services.minecraft.mods`, keyed by Modrinth slug. Empty `{}` includes the jar with defaults. Attrsets with fields configure the mod.

```nix
services.minecraft.mods = {
  fabric-api = {};
  lithium = {};
  distanthorizons.maxRenderDistance = 512;
};
```

The `modCatalog` option maps slugs to locked artifact sources. Set by the image base (from `common.json`) and version overlays (from `<version>.json`), then enriched through `ix.artifacts.attachArtifactSources`, which wraps each catalog entry's `{ url, hash }` in a `pkgs.fetchurl` derivation. The runtime resolves every key in `mods` to that derivation's store path.

### Mod modules

Mods with config files get a NixOS module at `modules/services/minecraft/mods/<name>.nix`. The module activates when `services.minecraft.mods.<slug>` is present, reads the user's attrset (with defaults), and generates `configFiles`.

```nix
# modules/services/minecraft/mods/distant-horizons.nix
{ config, lib, ... }:
let
  modCfg = config.services.minecraft.mods.distanthorizons or null;
  defaults = { serverSideLodGeneration = true; maxRenderDistance = 256; };
  merged = defaults // (if modCfg == null then {} else modCfg);
in
{
  config = lib.mkIf (modCfg != null) {
    services.minecraft.configFiles."DistantHorizons.toml" = {
      server = { inherit (merged) serverSideLodGeneration maxRenderDistance; };
    };
  };
}
```

Mods without config (lithium, krypton, chunky) do not need a module. The slug in `mods` is sufficient. Register mod modules in `modules/default.nix` under `minecraft.mods.<name>`.

### Config file format inference

`configFiles` keys are relative paths under `config/`. The serialization format is inferred from the file extension: `.toml`, `.json`, `.yaml`/`.yml`, `.properties`. Values are plain Nix attrsets. Mod modules never import `pkgs.formats` directly.

```nix
services.minecraft.configFiles."SomeMod.toml" = { section.key = "value"; };
services.minecraft.configFiles."other.yml" = { setting = true; };
```

### Writable vs read-only configs

Config files are symlinked to the Nix store (read-only). Some mods write to their config at runtime. If a mod needs a writable config, the mod module should copy instead of symlink. This is not yet implemented but is a known gap (see nix-minecraft's `files` vs `symlinks` pattern for reference).

## Cross-cutting helpers (`specialArgs.ix`)

Helpers shared across modules go in `lib/` and are exposed to every module through `specialArgs.ix`. Modules consume them as ordinary module args:

```nix
# modules/services/minecraft/paper.nix
{ ix, config, lib, ... }:
ix.mkMinecraftLoader {
  inherit config lib;
  name = "paper";
  dropDir = "plugins";
  extraOptions = { /* loader-specific options */ };
}
```

This is the only way modules in this repo reach helpers in `lib/`. **Never** use `../` or `../../` paths to climb out of `modules/`. Relative-path imports between modules and lib break the abstraction (modules become coupled to lib's filesystem layout) and make module files harder to relocate. If a helper needs to be shared, expose it through `specialArgs.ix`. The helper bundle is also part of the public flake `lib` output, so external users get the same surface.

## Module conventions

- Modules declare options and config. They never `imports` another module.
- Top-level options live under `services.<name>` for services, `ix.profiles.<name>` for profiles. Never reach into another module's namespace.
- Everything in `config` is wrapped in `mkIf cfg.enable`. The base profile is the only exception: it ships an enable flag so users can opt out.
- Module options take strings, paths, or packages — not factory arguments. Versioning belongs in `versions.nix`, not in a function wrapping the module.
- Public option names should describe the user's domain, not the storage mechanism. Prefer `services.minecraft.plugins` for Bukkit-family plugins and `services.minecraft.mods` for Fabric/NeoForge/Sponge mods; avoid vague plumbing names such as `extraJars` or `dropins` unless the storage mechanism is itself the concept.
- Cross-module helpers come from `specialArgs.ix`. No `..` paths.

## Path boundaries

Relative-up paths (`../`, `../../`, etc.) are usually an anti-pattern in tracked Nix code. They couple a file to a caller's current location instead of to the repo API boundary. Prefer named package sets, flake inputs, module options, or helpers exposed through `specialArgs.ix`. If a file needs something outside its directory tree, first ask which boundary should own that dependency and expose it there.

Relative paths to children or siblings inside the same package/module directory are fine. Relative-up paths are acceptable only when they are local, standard for the tool or ecosystem, and not reaching across a repo layer. The smell is climbing upward to reach across layers such as `images/` -> `packages/`, `modules/` -> `lib/`, or examples -> repo internals.

## Plugin conventions

Bukkit-family loaders (Paper, Folia, Purpur, Spigot) use `services.minecraft.plugins`. Empty `{}` resolves a pinned plugin by slug from `pluginCatalog`; an attrset with `src` installs a local or private plugin jar. Loader modules can contribute a catalog of common upstream plugins, so examples should not inline shared plugin URLs.

Fabric/NeoForge/Sponge-style artifacts stay in `services.minecraft.mods`. Keep mod and plugin catalogs near the image/module artifact plumbing, not in example fleets. Example fleets should read like intent: choose a server, select catalog plugins/mods by slug, and show local/private artifacts only when that is the point of the example.

## Image conventions

- Images set `ix.image.name`. They may set `ix.image.tag` (defaults to `latest`, or comes from `versions.nix`).
- Images compose by enabling services and adding packages. They do not declare options. They do not `imports` anything.
- Images stay version-agnostic when they have a `versions.nix`. The base file is what every variant shares; per-version data lives in the overlay.
- Use a single `services.<name>` block per service. Nest sub-options inside attrsets instead of writing scattered dotted assignments. Prefer `services.minecraft = { plugins = { luckperms = { }; claude-code-scoreboard = { ... }; }; };` over separate `services.minecraft.plugins.luckperms = { };` lines in examples.
- Options that are redundant with their namespace should be shortened. `services.minecraft.folia.version`, not `services.minecraft.folia.minecraftVersion`.
- Images should consume repo-local packages through `ix.packages`, not by importing `../../..` paths into `packages/`. Keep source-path ownership in the package set, following nixpkgs' `callPackage`/package-set style: modules and images choose package values, while package definitions own their filesystem layout.

## Example conventions

Examples are teaching material, not just tests. Add short comments for ix-specific ideas that a first-time reader will not infer from Nix alone: `deployment.switch.overrideInputs`, remote switch builds, fleet defaults, hot-reload behavior, and why an image name or tag is set.

In-repo examples that exercise this repo's library, modules, fleets, or pinned artifacts should be exposed from the root `flake.nix` as `apps`/`packages` and share the root `flake.lock`. Keep the actual fleet or image value in the example's `default.nix` so tests and root outputs can import it. Do not add a nested example `flake.lock` that pins this same repo; it will drift from the API under test.

Use a standalone example flake only when the example is intentionally a downstream consumer or template. In that case its inputs should look like a real external user's inputs (`github:indexable-inc/index`, registry refs, etc.), not `path:../..` or `git+file:../..` backedges into the parent checkout. Do not add example-local artifact inputs when the root flake already exposes the locked artifact through `ix.lib.artifacts`.

In fleet examples, `ix.image.name` usually defaults to the node name. Set it only when the replacement image should be named differently. Set `ix.image.tag` when the default `latest` would make plans or registry destinations ambiguous.

Comments should explain why a line exists, not restate Nix syntax. Prefer comments that answer "why is this needed in an ix fleet?" over comments that paraphrase the option name.

Use the ecosystem's normal project shape before inventing local scaffolding. Java examples should be Maven or Gradle projects with a `pom.xml`/build file, `src/main/java`, and resources; build them from Nix with `maven.buildMavenPackage` or the corresponding standard builder. Do not generate source files from Nix heredocs, vendor fake API stubs, or hand-roll classpaths when a normal build tool dependency is available.

Web examples should use ordinary frontend project structure too. Prefer TypeScript over JavaScript, split real UI into components/modules once a single file stops being clearer, and keep strict typechecking and ESLint in the default build path. For Svelte/Vite examples, `npm run build` should run `svelte-check`, ESLint, and the production bundle so `buildNpmPackage` enforces the same checks as local development. Use SvelteKit only when the example needs routes, server-side loading, endpoint handlers, or an app runtime; static status pages can stay Svelte/Vite.

Do not hide real source files inside Nix strings just to keep the file count small. If an example needs Java, scripts, config templates, or assets, put them in ordinary files with normal paths and keep the Nix derivation as the build recipe. Inline generated files are acceptable only for tiny machine-owned glue where reading a separate file would be worse.

At the same time, do not spray files around without a boundary. Group support code under a named subdirectory with a small `default.nix`, source files, and assets it needs. Reusable server code, plugins, and other composable artifacts belong under `packages/<family>/...`; image directories should compose those artifacts, not own unrelated build projects.

For self-contained support projects, filter at the project boundary instead of listing every source file. Prefer `lib.fileset.intersection (lib.fileset.gitTracked ./.) ./.` in the project-local `default.nix` so new tracked files under `src/`, `resources/`, Gradle metadata, or similar project-owned paths are included automatically while untracked build caches stay out of the store. Use explicit file lists only when the derivation intentionally consumes a small cross-cutting subset.

## Artifact inputs

Fetched artifacts (mod jars, server jars, plugins) belong at the point of use, not as flake inputs. Use `pkgs.fetchurl { url = ...; hash = ...; }` with an inline SRI hash kept beside the URL in the per-image catalog (`images/games/minecraft/mods/*.json` or the image's own data file). The `flake.nix` inputs list is reserved for things that genuinely participate in the flake graph: `nixpkgs` and tooling flakes that expose `lib`, `overlays`, or `packages`. A static URL is not a flake input. It is data, and data lives next to the code that reads it.

Earlier versions of this repo tracked every mod jar as a non-flake `inputs.artifact-*` URL so that `flake.lock` would own each `narHash`. That made `flake.nix` unreadable and centralized nothing useful: each entry still had to be edited individually, and the lock file became a churn-heavy diff for every routine mod bump. Prefer URL + SRI hash next to the catalog entry; the update tooling (`nix run .#update-mods`) regenerates both fields together.

Tracked Nix files must not contain `lib.fakeHash`, `lib.fakeSha256`, `lib.fakeSha512`, or placeholder hashes such as `sha256-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=`. Compute the real SRI hash before committing (e.g. `nix store prefetch-file --json <url> | jq -r .hash`) and store it with the URL.

## Target platform

All images run on AMD EPYC Gen 5 (Turin, Zen 5). `lib/ix-platform.nix` sets `nixpkgs.hostPlatform.gcc.arch = "znver5"` and `tune = "znver5"`, which propagates `-march=znver5 -mtune=znver5` to every package in the closure. This enables AVX-512, VNNI, and other Zen 5 instructions across the board.

Because the arch differs from the nixpkgs binary cache (generic x86_64), every package builds from source. This is intentional: these images run on known hardware and the build cost is paid once.

When adding new modules or packages, do not override compiler flags per-package. The base layer handles it globally. If a package needs arch-specific tuning beyond compiler flags (e.g. PostgreSQL `huge_pages`, JVM `-XX:+UseAVX`), set those in the module.

## Nix philosophy

- **Single source of truth.** `modules/default.nix` is the only place modules are listed; `lib.collect` derives the flat list from the nested attrset. Versions live next to the image in `versions.nix`, not in `flake.nix`. Hashes live next to URLs.
- **No backwards compat.** This repo is young and has no external consumers. Rename freely, change signatures, delete dead code. No shims, no aliases, no `// removed` comments, no feature flags for the old way. Update callers in the same change.
- **Auto-discover, don't enumerate.** `flake.nix` walks `images/`. Adding an image is `mkdir + edit one file`. Hand-wired registries rot.
- **DRY at the data layer, not the abstraction layer.** `inherit (pkgs) ...` over a wrapper helper. `lib.collect` over a parallel list. Don't introduce a function unless it has at least two callers; the minecraft-loader helper qualifies because eight loaders share the same shape.
- **Comments explain why, not what.** Headers say what each file is for and what's load-bearing (e.g. `maxLayers = 67` with the registry-cap rationale, base profile auto-enabled). Don't restate what the code obviously does.
- **Trust module merging.** Layer per-version overlays via the module system, not by passing args to factory functions. Service families (runtime + variants) compose through option slots, not through wrappers.
- **Pure eval.** No `builtins.currentSystem`, no `builtins.getEnv`, no `<nixpkgs>` channel refs, no `path:` flake refs. Every input flows through `flake.nix`.
- **Strict, named failures.** `lib.assertMsg` over bare `assert`. Required options have no default so misuse fails at eval with the option name. Two loaders enabled → module-merge conflict, not silently-last-wins.
- **Test useful invariants.** Eval tests should protect behavior that can regress across module boundaries, generated units, fleet rendering, artifact wiring, or security/runtime contracts. Do not assert facts that are already obvious from the same literal config being imported, such as exact example node names, hand-copied package allowlists from auto-discovery, or every field in an example's own `server.properties`. Flake evaluation already catches syntax and missing-output failures; tests should add signal beyond that.

## Nix practices to tighten

These are current repo habits that should not become defaults. When touching nearby code, improve the pattern in the same change. If the cleanup is wider than the task, file a narrow issue.

- **Typed module interfaces.** Avoid `types.attrs`, `types.attrsOf types.attrs`, and `types.anything` for domain data. Use `types.submodule`, `attrsOf (submodule ...)`, `types.enum`, `types.oneOf`, `types.nullOr`, or a `pkgs.formats.*.type` that matches the file being generated. Keep broad attrs only at true foreign-format boundaries, and name that boundary in the option description.
- **Filtered local sources.** Do not default to broad `src = ./project` or `src = ./site` inputs. Use `lib.fileset.toSource` with the smallest useful file set, usually `lib.fileset.intersection (lib.fileset.gitTracked ./.) (lib.fileset.unions [ ... ])`, or `lib.sources.cleanSourceWith` when a predicate is clearer. This avoids copying irrelevant files or secrets into the store and prevents rebuilds from unrelated local changes.
- **Executable paths.** Prefer `lib.getExe pkg` when the package's `meta.mainProgram` is correct, and `lib.getExe' pkg "program"` when the executable name is intentionally explicit. Add `meta.mainProgram` to repo packages that install a primary binary. Avoid scattering `"${pkg}/bin/foo"` through systemd units, apps, tests, and scripts unless there is no package value to pass around.
- **Checked builders.** Repo helpers that generate commands should run the strongest practical build-time validation by default, and callers should reuse those helpers instead of open-coding ad hoc wrappers. Examples: `ix.writeNushellApplication` runs Nu diagnostics, and `ix.writePythonApplication` runs basedpyright with required types. Keep the policy generic: add or extend one DRY helper when a language/tool needs validation, then use it everywhere.
- **Shell applications.** Use `ix.writeNushellApplication pkgs { ... }` for generated commands that call other programs, so runtime dependencies are explicit and Nu syntax is checked during the build. Do not use `writeShellApplication` or `writeShellScriptBin` in tracked Nix files. Tiny `writeShellScript` glue is acceptable only when the output is not a user-facing command.
- **Nushell wrappers.** Keep wrappers real Nu, not Bash hidden inside a Nu string. Use `def main [...args]`, structured values, lists with `...$args`, and `builtins.toJSON` for Nix-to-Nu literals. The wrapper helper must prepend declared runtime inputs while preserving the ambient `PATH`; fleet/app wrappers may need commands supplied by the caller, such as a freshly patched `ix` binary.
- **Scripts.** Prefer Nushell (`.nu`) over Bash for new non-trivial repo scripts, especially when the script parses JSON, builds structured output, or has enough branching that shell quoting becomes load-bearing. Package these scripts with `ix.writeNushellApplication pkgs { ... }` so runtime dependencies are explicit and Nu syntax is checked during the build. Bash is fine for tiny POSIX-style wrappers.
- **devShells.** Default to no devShell. Every package is already a shell: `nix develop nixpkgs#hello` enters `pkgs.hello`'s build environment, and `nix develop .#<package>` does the same for repo packages. `nix run .#lint` covers the lint case without a shell. Only add a `devShells.default` when there is real setup that no per-package environment provides (e.g. a shellHook that wires up `direnv` or seeds env vars). Accumulating a junk drawer of dev tools in `mkShell.packages` is the smell to avoid.
- **Pre-commit.** Do not depend on the `cachix/git-hooks.nix` framework when a one-line hook does the job. Use `.githooks/pre-commit` (chmod +x) that runs `nix flake check` or `nix run .#lint`, and have `.envrc` set `GIT_CONFIG_COUNT=1 / GIT_CONFIG_KEY_0=core.hooksPath / GIT_CONFIG_VALUE_0=./.githooks`. The lint app is the single source of truth; flake checks reuse it.

## Nix style (ast-grep enforced)

Run `nix run .#lint` before committing. It runs `nixfmt`, `statix`, `deadnix`, and the repo's ast-grep rules. Hard rules:

- No `with pkgs;` or `with lib;`. Use `inherit (pkgs) ...` or `lib.foo` directly.
- No `rec { }`. Use `let ... in` or `final/prev` instead.
- No `mkForce`. Resolve conflicts with priority composition or fix the module boundary.
- No `lib.recursiveUpdate`. Build the attrset in one place or use `lib.mkMerge`.
- No repeated parent keys in the same attrset. Group related assignments under one parent, e.g. `services.minecraft = { ...; };` or `environment.etc = { ...; };`, instead of several `services.minecraft.foo = ...;` lines in the same attrset.
- Prefer `inherit (source) name;` for direct field copies when the local name is the same. Avoid `name = source.name;` unless the assignment is clearer because it transforms or documents a boundary.
- No `builtins.currentSystem`, `builtins.getEnv`, `<nixpkgs>`, or `path:` flake refs.
- No `(import ./foo.nix)` inside `imports = [ ... ]`. NixOS auto-imports paths.
- No `..` paths inside `modules/`. Cross-cutting helpers come through `specialArgs.ix`.
- No `writeShellApplication` or `writeShellScriptBin`. Use `ix.writeNushellApplication pkgs { ... }` for user-facing commands and orchestrators.
- No bare `assert cond;`. Use `assert lib.assertMsg cond "why";`.
- No unused bindings. Use `_` for intentionally unused lambda arguments, remove unused module args, and run `deadnix --fail --no-lambda-pattern-names .` through `nix run .#lint`.
- `strictDeps = true` on every `mkDerivation`. `__structuredAttrs` is the nixpkgs default; do not set it explicitly.
- No artifact URLs in `flake.nix` inputs. Fetched assets (jars, plugins, server tarballs) go through `pkgs.fetchurl` at point of use with the URL and SRI hash held next to the catalog entry. Flake inputs are for flake-graph participants only (`nixpkgs`, tooling flakes).
- No fake hash helpers or placeholder hashes in tracked Nix files. Compute the real SRI hash first.
- No flat top-level `modules` flake output. Use `nixosModules.<name>` (standard schema) for module exports.
- Image target is x86_64-linux only. Host-visible flake package namespaces may include developer systems such as aarch64-darwin, but they should point at the same Linux image derivations rather than changing the image target.

## Issues

Keep issue bodies short. State the problem, the context, and the desired outcome. For bug reports, include a `To reproduce` section with the concrete command or steps that exposed the failure. Don't prescribe implementation steps or extra section headers unless explicitly asked.

When creating or editing GitHub issue bodies or comments, pass multiline text through a real multiline input path such as `--body-file -`, a temporary file, or an editor. Do not put escaped `\n` sequences inside a quoted `--body` string; they render literally on GitHub instead of becoming paragraph breaks.

When you hit a real bug, broken assumption, or unidiomatic pattern while working in this repo, file a GitHub issue right then (`gh issue create -R indexable-inc/index ...`). Don't batch and don't wait to be asked. One concrete observation per issue.

## Searching

Use `mgrep search -c {natural language}` to search the codebase. Do not use subagents for search.

## Linting

```
nix run .#lint
```

The repo wires `.githooks/pre-commit` to the same lint app via `.envrc`'s `core.hooksPath` override, so `direnv allow` is enough to get the pre-commit run on every `git commit`. CI runs `nix flake check`, which has a single `lint` check that calls the same derivation. There is no separate pre-commit framework to install.
