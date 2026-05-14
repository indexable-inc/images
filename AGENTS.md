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

## Trust model

Assume the agent running inside a VM has root and a goal it is optimizing for. It can install packages, edit `/etc`, restart services, flip `networking.firewall.enable`, and overwrite anything reachable from inside the guest. What it does not have by default: host API credentials, the ix CLI's host-side authority, registry-write tokens, or any secret you do not mount in. Design follows from that asymmetry. Anything that must hold against a misbehaving in-VM process belongs outside the VM, where the agent cannot reach it.

Concrete shapes this principle produces:

- **Network policy.** In-VM `networking.firewall.*` is convenience for a cooperative guest. The guest can disable it, so it is not a containment boundary. When a port must stay closed against a rogue guest, the enforcement point is a separate router/gateway VM the agent has no shell on, or one of ix's group/internet primitives. Inline `networking.firewall.allowedTCPPorts` is still the right place to *declare* intent (co-located with the service that needs the port); the enforcing layer reads that intent from outside.
- **Secrets.** Long-lived credentials (API keys, registry-write tokens, cloud creds) stay on the host or in a sibling VM that does not share a kernel with the agent. Mount only the short-lived, narrowly-scoped material the agent needs for the current task.
- **Snapshot and rollback authority.** The operator owns snapshot/rollback. An agent that can `ix snapshot` its own VM can paper over destructive behavior, so that surface belongs on the host side.
- **Image and switch-source authority.** The agent should not be able to switch its own VM to an unsigned or unreviewed image, or rewrite the source the VM was built from. That capability lives with the operator.

The "VM networking" section below is one specialization of this principle. Apply the same lens whenever a new feature lands: "if a rogue agent in this VM tried to subvert this, where does the rule actually live?"

## VM networking

Networking policy lives in the image, not in ix. ix exposes two primitives: VM group membership (east-west, which VMs can reach each other) and internet ingress/egress on or off (north-south, per direction). Per-port filtering, L7 rules, WAF, rate limiting, and mTLS termination belong in the image's NixOS config (`networking.firewall.*`, services in front of the workload) or in a user-built gateway VM. Do not push port allowlists or L7 features into ix; the matching rule on the ix side is recorded in `ix/AGENTS.md` under "Architecture that must not drift". If a service needs only some ports exposed, declare it in the image with `networking.firewall.allowedTCPPorts` or front it with a gateway VM. Treat in-image firewall config as cooperative-guest intent per the trust model above: if the policy must hold against a rogue agent inside the same VM, the enforcement layer must live on a separate gateway/router VM the agent cannot reach.

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
nix-rules/                                 # ast-grep lint rules
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
  default = "1.21.11-fabric";
  variants = {
    "1.21.11-fabric" = {
      loader = "fabric";
      version = "1.21.11";
      mods = [ "fabric-api" "spark" ];
    };
  };
in
{
  inherit default;
}
// lib.mapAttrs (tag: { loader, version, mods }: {
  ix.image.tag = tag;
  services.minecraft = {
    inherit version;
    mods = lib.genAttrs mods (_: { });
    ${loader}.enable = true;
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

## Doc-comments

Public helpers exposed through the flake `lib` output and through `specialArgs.ix` use `/** ... */` doc-comments placed immediately before the binding (RFC 0145). CommonMark inside. Document the API: what the helper does, the shape of its arguments, and the shape of its return. Implementation-only `#` comments stay for the "why" notes the rest of this document covers; doc-comments are the API surface. When adding a new helper next to a file with a single top-of-file block comment, lift the relevant prose into per-binding doc-comments.

```nix
/**
Build one self-contained OCI archive from a list of NixOS modules.

Runs `nixpkgs.lib.nixosSystem` over the platform config, OCI packaging, the
module registry, and any caller modules, then streams the toplevel into an
OCI archive. Returns the archive derivation.
*/
mkImage = args: (evalImageConfig args).ix.build.ociImage;
```

## Module conventions

- Modules declare options and config. They never `imports` another module.
- Top-level options live under `services.<name>` for services, `ix.profiles.<name>` for profiles. Never reach into another module's namespace.
- Everything in `config` is wrapped in `mkIf cfg.enable`. The base profile is the only exception: it ships an enable flag so users can opt out.
- Module options take strings, paths, or packages — not factory arguments. Versioning belongs in `versions.nix`, not in a function wrapping the module.
- Public option names should describe the user's domain, not the storage mechanism. Prefer `services.minecraft.plugins` for Bukkit-family plugins and `services.minecraft.mods` for Fabric/NeoForge/Sponge mods; avoid vague plumbing names such as `extraJars` or `dropins` unless the storage mechanism is itself the concept.
- Cross-module helpers come from `specialArgs.ix`. No `..` paths.
- Modules that render a structured config file expose typed settings backed by `pkgs.formats.*` with a freeform submodule (RFC 0042). The repo's `services.minecraft.configFiles` slot is the canonical pattern: keys are relative file paths, values are plain attrsets, and the format is inferred from the extension. Do not introduce stringly `extraConfig` options on new modules; concatenating strings can't merge same-key assignments, defeats `mkDefault`/`mkForce`, and makes values uninspectable.

## Path boundaries

Relative-up paths (`../`, `../../`, etc.) are usually an anti-pattern in tracked Nix code. They couple a file to a caller's current location instead of to the repo API boundary. Prefer named package sets, flake inputs, module options, or helpers exposed through `specialArgs.ix`. If a file needs something outside its directory tree, first ask which boundary should own that dependency and expose it there.

Relative paths to children or siblings inside the same package/module directory are fine. Relative-up paths are acceptable only when they are local, standard for the tool or ecosystem, and not reaching across a repo layer. The smell is climbing upward to reach across layers such as `images/` -> `packages/`, `modules/` -> `lib/`, or examples -> repo internals.

## Plugin conventions

Bukkit-family loaders (Paper, Folia, Purpur, Spigot) use `services.minecraft.plugins`. Empty `{}` resolves a pinned plugin by slug from `pluginCatalog`; an attrset with `src` installs a local or private plugin jar. The repo's plugin and mod catalogs (`ix.lib.artifacts.minecraft.*`, the per-version JSON catalogs under `images/games/minecraft/mods/`) are the shared surface that examples and images consume. Examples must not inline plugin or mod URLs and hashes; see the "Examples never own artifact data" rule under Example conventions.

Fabric/NeoForge/Sponge-style artifacts stay in `services.minecraft.mods`. Keep mod and plugin catalogs near the image/module artifact plumbing, not in example fleets. Example fleets should read like intent: choose a server, select catalog plugins/mods by slug, and show local/private artifacts only when that is the point of the example.

## Image conventions

- Images set `ix.image.name`. They may set `ix.image.tag` (defaults to `latest`, or comes from `versions.nix`).
- Images compose by enabling services and adding packages. They do not declare options. They do not `imports` anything.
- Images stay version-agnostic when they have a `versions.nix`. The base file is what every variant shares; per-version data lives in the overlay.
- Use a single `services.<name>` block per service. Nest sub-options inside attrsets instead of writing scattered dotted assignments. Prefer `services.minecraft = { plugins = { luckperms = { }; claude-code-scoreboard = { ... }; }; };` over separate `services.minecraft.plugins.luckperms = { };` lines in examples.
- Options that are redundant with their namespace should be shortened. `services.minecraft.folia.version`, not `services.minecraft.folia.minecraftVersion`.
- Images should consume repo-local packages through `ix.packages`, not by importing `../../..` paths into `packages/`. Keep source-path ownership in the package set, following nixpkgs' `callPackage`/package-set style: modules and images choose package values, while package definitions own their filesystem layout.

## DRY user-facing options

Every fact a user states in `config` should appear once. Apply this strictly when designing or extending a module, library, or helper: if a typical example sets `services.foo.version = "1.2.3"` and then also writes `services.foo.src = artifacts.foo."1.2.3"` and `services.foo.modCatalog = catalogs."1.2.3"`, the API is wrong. Restructure so the version drives the derived defaults and the example only mentions `1.2.3` once.

Three failure modes that justify a restructure:

- **Duplicated identifiers.** The same string appears in two or more option assignments in a typical config. Declare one canonical option (the version, hostname, slug, region) and have the rest default off it. Cross-option defaults (option B reads option A) are how you express derivation; use `defaultText` so the manual still shows the intent.
- **Per-call-site defaults.** Every caller writes the same `src = artifacts.foo.X;` line. Move the default onto the option (`default = artifacts.foo.X;`) so callers only override the exception. The library is where reachable defaults live; the example chooses among them by name.
- **Cargo-cult options.** A module declares `services.foo.bar.flavor` but no `config` block ever reads it. Delete the option. Forcing examples to set a value that nothing consumes is worse than not having the option, because it tricks readers into thinking the value matters.

Examples are the API's specification. Write the example first; if a single intent ("use Fabric 1.21.11") takes more than one line, the option set is too wide. A verbose example is a bug in the module's API, not in the example. The "Examples never own artifact data" rule below is the library-side complement: keep artifact data in `ix.lib.*` so examples can stay this short.

The repo has no external consumers, so renaming or collapsing options is free. Pay the migration cost (callers, tests, docs) in the same change.

## Example conventions

Examples are teaching material, not just tests. Add short comments for ix-specific ideas that a first-time reader will not infer from Nix alone: `deployment.switch.overrideInputs`, remote switch builds, fleet defaults, hot-reload behavior, and why an image name or tag is set.

### Examples never own artifact data

Examples must not inline URLs, hashes, or pinned version strings for fetched artifacts. Mod jars, plugin jars, server jars, datasets, JDKs, and source-fetched packages all live in the repo's library surface (`ix.lib.artifacts.*`, `ix.packages`, module options, generated catalogs under `images/`), and examples consume them by name. If an example needs an artifact the library does not expose yet, extend the library first: add the slug to the relevant catalog and regenerate it with `nix run .#update-mods`, add a new entry to `ix.lib.artifacts`, or grow the relevant module option. Then point the example at the named surface. Examples are consumer tests for whether the library is sufficiently specified. A missing entry is a gap in the repo; fix it in the library so every consumer benefits.

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

Fetched artifacts (mod jars, server jars, plugins, source trees) belong at the point of use, not as flake inputs. Use a `pkgs.*` fetcher with an inline SRI hash kept beside the source in the per-image catalog (`images/games/minecraft/mods/*.json` or the image's own data file). The `flake.nix` inputs list is reserved for things that genuinely participate in the flake graph: `nixpkgs` and tooling flakes that expose `lib`, `overlays`, or `packages`. A static URL is not a flake input. It is data, and data lives next to the code that reads it.

Pick the most specific `pkgs.*` fetcher for the source. `pkgs.fetchurl` is right for opaque single-file downloads (jars, zips, tarballs hosted at a stable URL). For source trees, prefer the upstream-specific fetcher so the derivation captures the right metadata and so future bumps go through one well-known field: `pkgs.fetchFromGitHub`, `pkgs.fetchFromGitLab`, `pkgs.fetchFromForgejo`, and friends for forge tarballs; `pkgs.fetchgit` for raw git refs; `pkgs.fetchzip` for archives that should be unpacked; `pkgs.fetchMavenArtifact`, `pkgs.fetchNpmDeps`, `pkgs.fetchCrate` for ecosystem artifacts. Do not use `builtins.fetchurl`, `builtins.fetchTarball`, `builtins.fetchGit`, or `builtins.fetchTree` in tracked Nix files: those run on eval, are not fixed-output derivations, do not substitute from a binary cache, and are banned in nixpkgs. The `pkgs.*` fetchers are (optionally) fixed-output derivations and only fetch at build time.

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
- **devShells.** Default to no devShell. Most of what people reach for one to do can be done without one. Env vars and PATH-bound tools belong in `.envrc` (direnv handles its own bootstrapping; do not wrap direnv in a `shellHook`). Editor-only tools (LSPs, formatters) belong in your editor config. A package that is genuinely required at build time belongs in that package's `nativeBuildInputs`. Per-package shells already exist: `nix develop nixpkgs#hello` enters `pkgs.hello`'s build environment, and `nix develop .#<package>` does the same for repo packages. The only place a `devShells.default` carries its weight is the unified-shell case: when you're regularly working across many of the repo's own packages and want one entry point, build it with `inputsFrom = [ pkg1 pkg2 ... ]` instead of accumulating a junk drawer in `mkShell.packages`.
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
- No artifact URLs in `flake.nix` inputs. Fetched assets (jars, plugins, server tarballs, source trees) go through a `pkgs.*` fetcher at point of use with the URL/ref and SRI hash held next to the catalog entry. Flake inputs are for flake-graph participants only (`nixpkgs`, tooling flakes).
- No `builtins.fetchurl`, `builtins.fetchTarball`, `builtins.fetchGit`, or `builtins.fetchTree` in tracked Nix files. Use the matching `pkgs.*` fetcher (`pkgs.fetchurl`, `pkgs.fetchzip`, `pkgs.fetchFromGitHub`/`pkgs.fetchgit`, etc.) so the fetch is a fixed-output derivation that can substitute from the cache.
- No fake hash helpers or placeholder hashes in tracked Nix files. Compute the real SRI hash first.
- No flat top-level `modules` flake output. Use `nixosModules.<name>` (standard schema) for module exports.
- Image target is x86_64-linux only. Host-visible flake package namespaces may include developer systems such as aarch64-darwin, but they should point at the same Linux image derivations rather than changing the image target.
- No stringly `extraConfig` / `extraSettings` options on new modules (RFC 0042). Structured config goes through `pkgs.formats.*` with a freeform submodule; the repo's `configFiles` slot is the canonical path.

## Issues

Keep issue bodies short. State the problem, the context, and the desired outcome. For bug reports, include a `To reproduce` section with the concrete command or steps that exposed the failure. Don't prescribe implementation steps or extra section headers unless explicitly asked.

When creating or editing GitHub issue bodies or comments, pass multiline text through a real multiline input path such as `--body-file -`, a temporary file, or an editor. Do not put escaped `\n` sequences inside a quoted `--body` string; they render literally on GitHub instead of becoming paragraph breaks.

When you hit a real bug, broken assumption, or unidiomatic pattern while working in this repo, file a GitHub issue right then (`gh issue create -R indexable-inc/index ...`). Don't batch and don't wait to be asked. One concrete observation per issue.

## Tests

Image and reusable package derivations expose their tests through `passthru.tests.<name>` (RFC 0119). A test that targets one image or one helper attaches to that derivation so `nix build .#<name>.passthru.tests.<test>` works and downstream tooling can iterate. Cross-image eval invariants stay in `tests/default.nix` and remain accessible through `checks.eval`. Tests do not run as part of the default image build.

Use `passthru.tests` for the lengthier or downstream-dependent checks (integration runs, fleet renders, end-to-end image boots). Keep `checkPhase` / `installCheckPhase` for the cheap inline checks that should always run on build.

## Searching

Use `mgrep search -c {natural language}` to search the codebase. Do not use subagents for search.

## Linting

```
nix run .#lint
```

The repo wires `.githooks/pre-commit` to the same lint app via `.envrc`'s `core.hooksPath` override, so `direnv allow` is enough to get the pre-commit run on every `git commit`. CI runs `nix flake check`, which has a single `lint` check that calls the same derivation. There is no separate pre-commit framework to install.
