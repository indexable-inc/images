# dev-tools

Miscellaneous first-party developer utilities, benchmarking, and worked
examples that do not belong to a larger domain. Nothing here runs in a deployed
service: these are local-developer and CI tools (a pretty `git log`, a TLS
reachability prober, a continuous-benchmarking framework) plus one standalone
GPU example. Each is its own `packages/<name>` crate; three are members of the
index Rust workspace surfaced as flake apps, one (`cuda-hello`) is a deliberately
detached crate with its own toolchain.

Read this page first, then the component page for the tool you are touching.

## Units

| package | crate / output | role |
| --- | --- | --- |
| `packages/git-log-pretty` | workspace member; `nix run .#git-log-pretty` | pretty `git log` viewer: commits ahead of `main` as colored file-icon trees, optional inline author avatars. See [git-log-pretty](git-log-pretty/overview.md). |
| `packages/ix-dev-diagnose` | workspace member; `nix run .#ix-dev-diagnose` | one-shot HTTPS reachability prober for `ix.dev`: writes a JSON diagnostic of DNS, TCP, TLS (dual-trust-store), and HTTP results. See [ix-dev-diagnose](ix-dev-diagnose/overview.md). |
| `packages/indexbench` | workspace member; `nix run .#indexbench` (CLI), `nix run .#bench` (perf job) | metric-centric continuous-benchmarking framework: micro + macro harnesses, durable history, statistical regression gate. See [indexbench](indexbench/overview.md). |
| `packages/cuda-hello` | standalone crate (not a workspace member; no flake app) | minimal CUDA kernel in pure Rust lowered to PTX via cuda-oxide; custom pinned nightly, Linux only. See [cuda-hello](cuda-hello/overview.md). |

## How it fits together

These are independent CLIs, not a layered subsystem. What they share is build
and packaging convention, not runtime code:

- **Three are standard workspace units.** `git-log-pretty`, `ix-dev-diagnose`,
  and `indexbench` each have a `default.nix` that calls
  `ix.cargoUnit.selectBinaryWithTests ix.rustWorkspace.units { binary = ...; }`
  and a `package.nix` with `flake = true; inRustWorkspace = true;
  passthruTests = true;`, so each gets a `nix run .#<name>` app and runs its
  test target through the shared cargo-unit graph
  (`packages/git-log-pretty/default.nix:3`, `packages/indexbench/default.nix:3`,
  `packages/ix-dev-diagnose/default.nix:3`). `mainProgram` is set so `lib.getExe`
  resolves the binary.
- **`cuda-hello` is the exception by design.** Its `Cargo.toml` has an empty
  `[workspace]` table to detach it from the repo workspace, it has no
  `package.nix`, and its toolchain is a pinned nightly the workspace toolchain
  cannot build, so the root `nix flake check` never touches it
  (`packages/cuda-hello/Cargo.toml:1-15`). It ships its own `flake.nix` that
  provides only dev shells.
- **Terminal-output crates are reused, not reimplemented.** `git-log-pretty`
  styles through sibling workspace crates `terminal-theme` (light/dark
  detection), `kitty` (graphics protocol), `github-avatar` (login resolution and
  avatar fetch), plus the external `devicons` for file glyphs
  (`packages/git-log-pretty/Cargo.toml:11-29`). Those crates belong to other
  domains; this domain only consumes them.
- **`indexbench` is consumed by Nix, not just run by hand.** `ix.mkBenchSuite`
  (`lib/util/bench.nix`) turns a data description of a suite into an `app` (a
  `nix run`-able perf job) and an optional `check` (a hermetic `nix flake check`
  that gates a deterministic allocation count). The repo's own self-demo suite
  is wired in `lib/per-system.nix:404-420`.

## Invariants

- **Exit code is the contract, where there is one.** `indexbench run` and
  `indexbench assert` exit non-zero on a regression / over-budget metric, which
  is the CI gate (`packages/indexbench/src/main.rs:263-267`,
  `:353-357`). `ix-dev-diagnose` is the opposite: it always returns success and
  signals reachability only through its printed `success`/`failure` line and the
  JSON `summary.ok`, never the process exit code
  (`packages/ix-dev-diagnose/src/main.rs:271-293`).
- **Read a local repo without system libraries.** Both git-aware tools depend on
  `git2` with `default-features = false` plus `vendored-libgit2`, so the build
  compiles bundled libgit2 with `cc` and needs no system libgit2, openssl, or
  cmake (`packages/git-log-pretty/Cargo.toml:18-23`).
- **Reproducible vs sandbox-sensitive metrics.** `indexbench` keeps deterministic
  metrics (allocation counts) usable as `nix flake check`s and routes
  timing/RSS to a perf job, because only the former are stable inside the Nix
  sandbox (`packages/indexbench/src/lib.rs:24-26`).

## Glossary

- **workspace unit / cargo-unit**: a `packages/<name>` crate built through
  `ix.cargoUnit.selectBinaryWithTests` over the shared `rustWorkspace.units`
  graph; gives one binary plus a passthru test derivation.
- **flake app**: an output exposed by `package.nix` `flake = true`, runnable as
  `nix run .#<name>`.
- **metric**: in `indexbench`, any named number with a unit and a direction
  (`lower_is_better`); the framework's central abstraction, not "time".
- **distributional / deterministic metric**: a metric with per-iteration
  `samples` (gets a statistical test) versus one without (exact compare).
- **regression gate**: `indexbench`'s rule that exits non-zero when a metric
  worsened past its regime's threshold; see [indexbench](indexbench/overview.md).
- **recording verifier**: `ix-dev-diagnose`'s custom rustls verifier that records
  trust outcomes against two root stores but never fails the handshake, so the
  full certificate chain is captured even when verification fails.
- **kitty Unicode placeholder**: the terminal graphics mechanism `git-log-pretty`
  uses to draw author avatars as ordinary scrolling text cells.
- **PTX**: NVIDIA's GPU assembly; `cuda-hello` lowers a Rust `#[kernel]` to PTX
  via cuda-oxide, with no GPU needed to compile.
- **cuda-oxide**: NVIDIA's experimental rustc codegen backend (`cargo oxide`)
  that compiles Rust to CUDA PTX.

## Components

| component | page | what |
| --- | --- | --- |
| git-log-pretty | [git-log-pretty/overview.md](git-log-pretty/overview.md) | commits-ahead-of-main `git log` with file-icon trees and kitty avatars |
| ix-dev-diagnose | [ix-dev-diagnose/overview.md](ix-dev-diagnose/overview.md) | JSON HTTPS reachability diagnostics for ix.dev |
| indexbench | [indexbench/overview.md](indexbench/overview.md) | continuous benchmarking: harnesses, durable history, regression gate |
| cuda-hello | [cuda-hello/overview.md](cuda-hello/overview.md) | minimal pure-Rust CUDA kernel compiled to PTX (standalone) |
