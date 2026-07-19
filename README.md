# index

<p align="center">
  <img src=".github/readme/unblocked.svg" alt="agents write code faster than upstream review can take it; index lands the change on main now and fans it out to everyone" width="720">
</p>

Agents made writing code cheap. Shipping it is still expensive: review queues, maintainer bus factors, projects that will not take AI-written patches at all. The bottleneck moved from writing the change to landing it.

index is the other path: one Nix monorepo, in the spirit of [nixpkgs](https://github.com/NixOS/nixpkgs) and [Raycast extensions](https://github.com/raycast/extensions), where every change lands on main now and upstream can take it someday. Inside: [packages](packages/), patched toolchains ([Nix](packages/nix/nix/), [Clippy](packages/llm-clippy/)), [NixOS and Home Manager modules](modules/), [VM images](images/), and [CI](.github/workflows/). See the [philosophy](https://indexable-inc.github.io/index/philosophy/) for more.

## Why

<p align="center">
  <img src=".github/readme/flywheel.svg" alt="the flywheel: a change lands once, it reaches everything, everything improves, and the next change is cheaper; the loop spins faster and faster" width="720">
</p>

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

You download binaries instead of compiling them. CI builds on the [ix.dev](https://ix.dev) cluster, close to 1,000 vCPUs, and pushes every closure to `cache.ix.dev`, prebuilt for Linux and cross-compiled for macOS.

## Stories

The fastest way to get what this repo is for: short case examples, each a two-minute read with a diagram.

1. [Your whole team's Claude, from one flake](https://indexable-inc.github.io/index/stories/manage-claude-with-nix/): the agent binary, prompt, tools, permissions, and MCP servers, pinned in code.
2. [Add a tool once, everyone gets it](https://indexable-inc.github.io/index/stories/add-a-tool-once/): a small utility stops dying on the laptop it was born on.
3. [Your Mac never compiles](https://indexable-inc.github.io/index/stories/mac-never-compiles/): the Linux fleet cross-compiles macOS binaries your laptop just downloads.
4. [Every session becomes searchable memory](https://indexable-inc.github.io/index/stories/searchable-history/): shell and agent history from every machine, one semantic index.
5. [CI builds each crate exactly once](https://indexable-inc.github.io/index/stories/build-each-crate-once/): the Rust workspace as a per-crate build DAG.
6. [A thousand agents, one Elixir kernel](https://indexable-inc.github.io/index/stories/elixir-agent-kernel/): agents work through supervised, fleet-federated workspaces on a runtime built for that shape.
