# index

<p align="center">
  <img src=".github/flywheel.svg" alt="The flywheel: a patch lands once, the whole graph rebuilds, everything ships prebuilt for Linux and macOS, agents migrate every consumer" width="680">
</p>

Everything we build and depend on, in one repo, as one build graph.

index is a Nix monorepo: ~85 packages, forked toolchains (our own Nix, our own Clippy), NixOS and Home Manager modules, VM images, and the CI that builds it all. Think the Raycast extensions monorepo, but for an entire stack.

## Why

**A change lands once and reaches everything.** Patch a compiler, fix a library, tighten a lint: the whole graph rebuilds against it. No package is quietly running last year's version of anything.

**Never blocked on upstream.** Patches live here, next to the code that needs them. If upstream is slow, wrong, or abandoned, we ship anyway. No dependency has a bus factor we don't control.

**One bar for everything.** Security scans, license checks, and our Clippy fork's custom lints run over the whole graph. Add a rule and every crate meets it, in the same change.

**No stable internal APIs, on purpose.** Every consumer of every internal API lives in this repo, and agents make repo-wide refactors cheap. So an API is free to be correct instead of compatible: change it, migrate every call site, land one commit.

**Prebuilt everywhere.** CI builds the graph for Linux and cross-compiles it for macOS, then pushes to `cache.ix.dev`. You download binaries instead of compiling them.

## The flywheel

These compound. Better tools make the agents better; better agents make sweeping changes cheap; cheap changes keep everything at the newest bar; and the improved tools feed straight back in. The repo gets easier to improve the more it improves. That is the point.

## Layout

| dir | what |
|---|---|
| `packages/` | the tools (85 and counting) |
| `modules/` | NixOS + Home Manager modules |
| `lib/` | build machinery (`cargo-unit`, package sets) |
| `images/` | VM images for the ix fleet |
| `examples/` | consuming index from your own flake |
