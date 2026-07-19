# index

<p align="center">
  <img src=".github/readme/flywheel.svg" alt="the flywheel: a change lands once, it reaches everything, everything improves, and the next change is cheaper" width="720">
</p>

One build graph for everything.

index is a Nix monorepo in the spirit of [nixpkgs](https://github.com/NixOS/nixpkgs) and [Raycast extensions](https://github.com/raycast/extensions): packages, patched toolchains (Nix, Clippy), NixOS and Home Manager modules, VM images, and the CI that builds them all live together, so they move together.

## Why

### A change lands once and reaches everything

<p align="center">
  <img src=".github/readme/one-graph.svg" alt="one patch fans out to every package in the graph" width="720">
</p>

Patch a compiler, fix a library, tighten a lint: every package in the graph rebuilds against it. Nothing quietly runs last year's version of anything.

### Never blocked on upstream

<p align="center">
  <img src=".github/readme/upstream.svg" alt="patches live in the repo next to the code; no waiting on upstream" width="720">
</p>

Patches live here, next to the code that needs them. A slow, wrong, or abandoned upstream never blocks a fix, and no dependency has a bus factor outside the repo.

### One standard for everything

<p align="center">
  <img src=".github/readme/one-standard.svg" alt="custom clippy lints, cve scans, and license checks cover the whole graph" width="720">
</p>

Security scans, license checks, custom Clippy lints: every rule runs over the whole graph. Add a rule and every package meets it, in the same change.

### No stable internal APIs, on purpose

<p align="center">
  <img src=".github/readme/refactor.svg" alt="an api change migrates every call site in one commit" width="720">
</p>

Every consumer of every internal API lives here, and agents make repo-wide refactors cheap. An API is free to be correct instead of compatible: change it, migrate every call site, land one commit.

### Prebuilt everywhere

<p align="center">
  <img src=".github/readme/prebuilt.svg" alt="ci pushes builds to cache.ix.dev, prebuilt for linux and cross-compiled macos" width="720">
</p>

CI builds the graph for Linux, cross-compiles it for macOS, and pushes to `cache.ix.dev`. Binaries download instead of compile.

## The flywheel

These compound. Better tools make agents more capable; capable agents make sweeping changes cheap; cheap changes keep everything at the newest standard; the improved tools feed straight back in. The repo gets easier to improve the more it improves.

## Layout

| dir | what |
|---|---|
| `packages/` | the tools |
| `modules/` | NixOS + Home Manager modules |
| `lib/` | build machinery (`cargo-unit`, package sets) |
| `images/` | VM images |
| `examples/` | consuming index from your own flake |
