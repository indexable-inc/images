# Packaging

Repo-owned Nix repackages of third-party tools. Each package under `packages/`
documented here is a thin wrapper around an upstream tool (a CLI, a TUI, a JVM
distribution, or a GUI bundle) that this repo rebuilds with its own baked-in
defaults, patches, source pins, or NixOS fixups. Nothing here is original
software: the value is the deltas (forced flags, config injection, ELF
patching, version pinning) layered on top of the upstream artifact so it runs
correctly and consistently inside this fleet.

Read this page first for the shared conventions (how a wrapper is structured,
how it is wired into the flake, how versions are pinned and bumped), then the
per-package page for the specific deltas that package adds. The intake policy
these packages follow is `agent-context/sections/13-dependency-intake.md`.

## Packages

| package | upstream | what this repo changes |
| --- | --- | --- |
| [btop](btop/overview.md) | btop system monitor | `overrideAttrs` swaps `src` to a repo fork pinned by flake input rev |
| [claude-code](claude-code/overview.md) | Anthropic Claude Code CLI | baked flags/env/settings/MCP/system-prompt/hooks via the `config-launch` launcher; signed-manifest version pin |
| [codex](codex/overview.md) | OpenAI Codex CLI (nixpkgs) | baked forced + soft `-c` config via `config-launch`; additive flake output only |
| [dia](dia/overview.md) | The Browser Company Dia browser | unpacks the signed `.dmg` verbatim; manifest pin + updater; aarch64-darwin only |
| [launchk](launchk/overview.md) | mach-kernel/launchk launchd TUI | `buildRustPackage` from a pinned flake-input rev; bindgen + `git_version!()` fixups; Darwin only |
| [spark-gluten](spark-gluten/overview.md) | Apache Gluten Velox bundle | explodes the bundle jar, `autoPatchelf`s the CentOS-7 native libs for NixOS, repacks; x86_64-linux only |
| [spark-hive](spark-hive/overview.md) | Apache Spark hadoop3+Hive | full distribution, wrappers pin JDK 17 + TZDIR; x86_64-linux only |
| [tonbo-artifacts](tonbo-artifacts/overview.md) | Tonbo Artifacts CLI | installs a prebuilt binary pinned by URL rev; x86_64-linux only |
| [tmux](tmux/overview.md) | tmux | `symlinkJoin` + `wrapProgram -f` to bake a truecolor/mouse/vi config |
| [vineflower](vineflower/overview.md) | Vineflower Java decompiler | downloads the release jar, writes a `java -jar` launcher; inline version pin |
| [yc](yc/overview.md) | Y Combinator CLI | installs prebuilt per-platform binaries; manifest pin + updater (no provenance check) |

## How a wrapper is structured

Every package is one directory under `packages/<id>/`. The recurring files:

- `default.nix`: the derivation. It does one of four things (see the per-package
  pages for which): rebuild upstream with a swapped source (`overrideAttrs`,
  btop), build from source (`rustPlatform.buildRustPackage`, launchk), wrap an
  existing package (`symlinkJoin` + a wrapper, tmux/codex/claude-code), or
  install a prebuilt artifact (`stdenv.mkDerivation` + `fetchurl`, dia, yc,
  tonbo-artifacts, vineflower, spark-gluten, spark-hive). The baked defaults and
  patches live here.
- `package.nix`: registry metadata, an attrset (`packages/registry.nix:30-39`
  lists the allowed keys). The load-bearing keys:
  - `id`: the package name (and default flake/overlay attr name, default
    package-set attr path).
  - `packageSet` / `flake` / `overlay`: where the package surfaces. `true` uses
    the `id` as selector; an attrset can set `systems` to gate platforms (so
    `nix flake check` does not try to build an off-platform package) or override
    the attr name/path (`packages/registry.nix:55-100`).
  - `updateScript = true`: declares the package exposes a
    `passthru.updateScript` and joins the repo-wide `update` aggregator
    (`packages/registry.nix:136-140`, `161-162`).
- `manifest.json` (prebuilt-binary packages only): the generated
  `{ version, hash }` or `{ version, platforms.<sys>.{slug,hash} }` pin, read
  with `lib.importJSON` and never hand-edited. Present for claude-code, dia, yc.
- update logic (manifest packages only): a `passthru.updateScript`
  (`writeNushellApplication`) that refetches the upstream pointer and rewrites
  `manifest.json`. claude-code factors it into `update.nix`; dia and yc inline
  it in `default.nix`.

## How a package is wired into the flake

The registry discovers every directory containing a `package.nix`
(`packages/registry.nix:24`, `116-141`); there is no central list to edit.

- Package set: `lib/packages.nix` calls each `default.nix` with `pkgs`, the
  `ix` helper bundle, and `repoPackages` (the package set itself, a lazy
  fix-point, so one package can depend on a sibling by id without a flat merge
  that would shadow nixpkgs attrs: `lib/packages.nix:38-57`). This is the eval
  context where sibling-dependent defaults (the baked `index` MCP server, the
  `config-launch` launcher) are available.
- Flake output: `lib/per-system.nix:537-541` turns every `flake`-enabled entry
  into `packages.<system>.<attrName>`, so `nix run .#<name>` and
  `nix build .#<name>` work (no `apps` entry needed; the wrapper sets
  `meta.mainProgram`).
- Overlay: `overlay`-enabled entries also surface as `pkgs.<name>` for NixOS
  modules (`lib/overlay.nix`). Some packages (codex) are deliberately
  flake-only so `pkgs.<name>` stays the plain nixpkgs version.
- Source pinning: a wrapper pins its upstream three ways. Flake-input rev
  (`flake.nix` `*-src` inputs, exposed as `ix.<name>Src`): btop, launchk. Inline
  `version` + `hash` in `default.nix`: vineflower, tonbo-artifacts, spark-gluten,
  spark-hive. Generated `manifest.json` + updater: claude-code, dia, yc.
- Bumping a manifest pin: `nix run .#<id>.updateScript -- [version]` refreshes
  that one package; `nix run .#update` runs every `updateScript = true`
  package's updater in parallel via `dag-runner`
  (`lib/per-system.nix:461-501`), and `update.yml` runs it hourly into one PR.
  Inline-pinned packages bump by editing `version` and refreshing `hash` with
  `nix-prefetch-url`; flake-input packages bump by `nix flake update <input>`.

## Conventions

- `meta.platforms` and the `package.nix` `systems` gates are kept in sync so an
  off-platform `nix flake check` never forces a build nixpkgs refuses to
  evaluate (`packages/launchk/package.nix:3-14`,
  `packages/spark-gluten/package.nix:3-12`). Linux-only or macOS-only packages
  list only their systems.
- A proprietary/unfree vendored binary omits `meta.license` rather than tagging
  it `licenses.unfree`, because the per-system flake package set evaluates
  nixpkgs without `allowUnfree` and the tag would block `nix run .#<name>`
  (`packages/claude-code/default.nix:489-492`, `packages/dia/default.nix:96-99`,
  `packages/yc/default.nix:100-103`).
- A prebuilt binary sets `sourceProvenance = [ lib.sourceTypes.binaryNativeCode ]`
  (or `binaryBytecode` for JVM), `dontStrip` where stripping would corrupt the
  artifact (claude-code's Bun trailer, spark-gluten's vendored `.so`), and
  `strictDeps = true`.
- The shared launcher `config-launch` (`packages/config-launch`, a compiled Rust
  binary) is the injection mechanism for the two config-baking CLIs: it reads a
  baked `IX_LAUNCH_SPEC` JSON (target binary, forced flags, soft defaults, env,
  PATH), does per-key presence detection against the user's config, then execs
  the real binary preserving argv0. claude-code and codex both use it.

## Glossary

- **wrapper / repackage**: a repo-owned derivation that takes an upstream
  artifact and adds deltas (flags, config, patches, source pin). Every package
  on this page is one.
- **baked default**: a flag, env var, or settings key the wrapper injects on
  every invocation. Forced (always wins) or soft (only when the user has not
  set it).
- **config-launch / launch spec**: the shared Rust launcher and its
  `IX_LAUNCH_SPEC` JSON. The mechanism claude-code and codex use to inject
  config without editing the read-only store binary.
- **manifest pin**: a generated `manifest.json` holding a version and SRI
  hash(es), read with `lib.importJSON`, refreshed only by the package's
  `updateScript`.
- **updateScript**: a `passthru.updateScript` that refetches an upstream
  pointer and rewrites `manifest.json`. Flagged in `package.nix`
  (`updateScript = true`) to join `nix run .#update`.
- **autoPatchelf**: the nixpkgs hook that rewrites a foreign-built ELF's
  interpreter and rpath to Nix store paths so it loads on NixOS (used by
  spark-gluten, yc Linux).
- **flake-input rev**: an upstream source pinned as a `flake = false` input in
  `flake.nix` at a git commit, surfaced to a package as `ix.<name>Src`.
- **soft vs forced**: a soft default is injected only when the user's config
  does not already set that exact key; a forced setting is applied
  unconditionally (a wrapper invariant the user must not silently lose).

## Components

| component | page | what |
| --- | --- | --- |
| btop | [btop/overview.md](btop/overview.md) | btop rebuilt against a repo fork source |
| claude-code | [claude-code/overview.md](claude-code/overview.md) | Claude Code CLI with baked flags/env/settings/MCP/prompt/hooks |
| codex | [codex/overview.md](codex/overview.md) | OpenAI Codex CLI with baked `-c` config defaults |
| dia | [dia/overview.md](dia/overview.md) | Dia browser `.dmg` repackaged verbatim, manifest + updater |
| launchk | [launchk/overview.md](launchk/overview.md) | launchd-observer Rust TUI built from a pinned rev |
| spark-gluten | [spark-gluten/overview.md](spark-gluten/overview.md) | Gluten Velox bundle, native libs patched for NixOS |
| spark-hive | [spark-hive/overview.md](spark-hive/overview.md) | Spark hadoop3+Hive distribution, JDK 17 wrappers |
| tonbo-artifacts | [tonbo-artifacts/overview.md](tonbo-artifacts/overview.md) | prebuilt Tonbo Artifacts CLI binary |
| tmux | [tmux/overview.md](tmux/overview.md) | tmux wrapped with a truecolor/mouse/vi config |
| vineflower | [vineflower/overview.md](vineflower/overview.md) | Vineflower decompiler jar with a `java -jar` launcher |
| yc | [yc/overview.md](yc/overview.md) | YC CLI prebuilt binaries, manifest + updater |
