# index

<p align="center">
  <img src=".github/readme/flywheel.svg" alt="the flywheel: a change lands once, it reaches everything, everything improves, and the next change is cheaper; the loop spins faster and faster" width="720">
</p>

One build graph for everything.

index is a Nix monorepo in the spirit of [nixpkgs](https://github.com/NixOS/nixpkgs) and [Raycast extensions](https://github.com/raycast/extensions): packages, patched toolchains (Nix, Clippy), NixOS and Home Manager modules, VM images, and CI, together in one graph.

## Why

### A change lands once and reaches everything

<p align="center">
  <img src=".github/readme/one-graph.svg" alt="one patch fans out to every package in the graph" width="720">
</p>

Patch a compiler, fix a library, tighten a lint: nothing quietly runs last year's version of anything.

### Never blocked on upstream

<p align="center">
  <img src=".github/readme/upstream.svg" alt="a fix lands in this repo now; upstream can take it someday" width="720">
</p>

Patches live next to the code that needs them. No dependency has a bus factor outside the repo.

### One standard for everything

<p align="center">
  <img src=".github/readme/one-standard.svg" alt="clippy, cve scan, and licenses applied to every package in the graph" width="720">
</p>

Add a rule and every package meets it, in the same change.

### No stable APIs required

<p align="center">
  <img src=".github/readme/refactor.svg" alt="an api change migrates every call site in one commit" width="720">
</p>

Every consumer of an API lives in this repo, and agents make repo-wide refactors cheap, so an API can be correct instead of compatible.

### Prebuilt everywhere

<p align="center">
  <img src=".github/readme/prebuilt.svg" alt="ci pushes builds to cache.ix.dev, prebuilt for linux and cross-compiled macos" width="720">
</p>

You download binaries instead of compiling them.

## Layout

| dir | what |
|---|---|
| `packages/` | the tools |
| `modules/` | NixOS + Home Manager modules |
| `lib/` | build machinery (`cargo-unit`, package sets) |
| `images/` | VM images |
| `examples/` | consuming index from your own flake |
