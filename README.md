# index

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset=".github/readme/globe-dark.svg">
    <img src=".github/readme/globe.svg" alt="a spinning globe: the whole world, rendered as braille text" width="480">
  </picture>
</p>

[ix.dev](https://ix.dev) rents out cloud machines where you pay for what you use. index defines everything those machines run: the apps, the libraries they are built from, and the tools that build them, compilers included. Inside: [packages](packages/), patched toolchains ([Nix](packages/nix/nix/), [Clippy](packages/llm-clippy/)), [NixOS and Home Manager modules](modules/), [VM images](images/), and [CI](.github/workflows/).

Every dependency here, down to the compiler, is something this repo can patch: the moment upstream falls short, index forks it. Forking has always been a bad trade, a same-day fix paid for by re-applying it by hand on every upstream update, forever, which is why forks rot and why everyone waits on review queues instead. index bets that [agents have flipped the trade](https://indexable-inc.github.io/index/philosophy/): they carry the upkeep, the build graph checks their work, and whatever a fix breaks surfaces within the hour as a failed check.

## What you get

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset=".github/readme/flywheel-dark.svg">
    <img src=".github/readme/flywheel.svg" alt="the flywheel: a change lands once, it reaches everything, everything improves, and the next change is cheaper; the loop spins faster and faster" width="720">
  </picture>
</p>

### A change lands once and reaches everything

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset=".github/readme/one-graph-dark.svg">
    <img src=".github/readme/one-graph.svg" alt="one patch fans out to every package in the graph" width="720">
  </picture>
</p>

Patch a compiler, fix a library, tighten a lint: nothing quietly runs last year's version of anything.

### Never blocked on upstream

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset=".github/readme/upstream-dark.svg">
    <img src=".github/readme/upstream.svg" alt="a fix lands in this repo now; upstream can take it someday" width="720">
  </picture>
</p>

Patches live next to the code that needs them: a slow review queue, or a project that refuses AI-written patches outright, blocks nothing. Upstream can adopt the fix whenever it wants.

### One standard for everything

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset=".github/readme/one-standard-dark.svg">
    <img src=".github/readme/one-standard.svg" alt="clippy, cve scan, and licenses applied to every package in the graph" width="720">
  </picture>
</p>

Add a rule and every package meets it, in the same change.

### No stable APIs required

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset=".github/readme/refactor-dark.svg">
    <img src=".github/readme/refactor.svg" alt="an api change migrates every call site in one commit" width="720">
  </picture>
</p>

Every consumer of an API lives in this repo, and agents make repo-wide refactors cheap, so an API can be correct instead of compatible.

### Prebuilt everywhere

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset=".github/readme/prebuilt-dark.svg">
    <img src=".github/readme/prebuilt.svg" alt="ci pushes builds to cache.ix.dev, prebuilt for linux and cross-compiled macos" width="720">
  </picture>
</p>

You download binaries instead of compiling them. CI builds on the [ix.dev](https://ix.dev) cluster, close to 1,000 vCPUs, and pushes every closure to `cache.ix.dev`, prebuilt for Linux and cross-compiled for macOS.

## Stories

The fastest way to get what this repo is for: short case examples, each a two-minute read with a diagram.

1. [Your whole team's Claude, from one flake](https://indexable-inc.github.io/index/stories/manage-claude-with-nix/): the agent binary, prompt, tools, permissions, and MCP servers, pinned in code.
2. [Add a tool once, everyone gets it](https://indexable-inc.github.io/index/stories/add-a-tool-once/): a small utility stops dying on the laptop it was born on.
3. [Your Mac never compiles](https://indexable-inc.github.io/index/stories/mac-never-compiles/): the Linux fleet cross-compiles macOS binaries your laptop just downloads.
4. [Every session becomes searchable memory](https://indexable-inc.github.io/index/stories/searchable-history/): shell and agent history from every machine, one semantic index.
5. [CI builds each crate exactly once](https://indexable-inc.github.io/index/stories/build-each-crate-once/): the Rust workspace as a per-crate build DAG.
6. [A thousand agents, one Elixir kernel](https://indexable-inc.github.io/index/stories/elixir-agent-kernel/): agents work through supervised, fleet-federated workspaces on a runtime built for that shape.
