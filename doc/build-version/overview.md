# build-version

`packages/build-version` is a tiny library crate that formats a binary's
`--version` line from build metadata a Nix wrapper stamps into the environment,
so every ix tool reports its revision, commit date, and how long ago it was built
in one consistent shape. No binary, no flake output: it is a dependency of the
tools that want a stamped version.

## Purpose

A reproducible build has no wall-clock compile time, so the "when" is the flake's
commit time, not the build time. The Nix wrapper sets the revision and commit
epoch in the environment; this crate reads them at startup and renders the line.
"How long ago" is computed at run time against the current clock, which is why it
cannot be baked into the binary and rides the env instead (`src/lib.rs:1-8`).

## Public surface (`src/lib.rs`)

- `version_static(crate_version: &str) -> &'static str` (`:41`): the entry point.
  Hand the result straight to clap's `Command::version`. When the wrapper env is
  present it returns e.g. `0.1.0 (7e42ccdb1882, 2026-06-07, 2 days ago)`; outside
  the packaged wrapper (a dev `cargo run`) the env is unset and it returns the
  bare crate version. Computed once per process via a `OnceLock`, matching how
  clap reads the version once at startup.
- `stamp(rev, epoch: Option<i64>, now: SystemTime) -> String` (`:69`): builds the
  parenthesised stamp. Split out so it is testable without touching the
  environment or the wall clock. Abbreviates the revision to 12 chars
  (`SHORT_REV_LEN`, `:30`); degrades to the short revision alone when no epoch is
  known, and treats `epoch == 0` as unknown (the `revEpoch ? 0` sentinel for a
  non-git eval, so it never prints `1970-01-01, 56 years ago`, `:71-76`).
- `humanize_ago(seconds: i64) -> String` (`:96`): renders an elapsed span as
  `just now` / `5 minutes ago` / `2 days ago` / `1 year ago`. Spans under a
  minute and negative spans (build clock ahead of ours) collapse to `just now`.

## Environment contract

- `REV_ENV = "IX_BUILD_REV"` (`:23`): the build's flake revision: full git SHA on
  a clean tree, `<sha>-dirty` when dirty, `dev` off a non-git source. Mirrors
  `ix.rev`.
- `EPOCH_ENV = "IX_BUILD_EPOCH"` (`:27`): the build's commit time as unix epoch
  seconds (`self.lastModified`). Mirrors `ix.revEpoch`.

These are the shared names every ix tool reads. The Nix wrapper for
[nix-web-monitor](../nix-web-monitor/overview.md) is the worked example of setting
them (`packages/nix/nix-web-monitor/server/default.nix:59-62`), and its
`server/src/main.rs:107` is a worked example of consuming `version_static`.

## Build and packaging

`package.nix`: `inRustWorkspace`, `passthruTests`. Library crate, no flake
output. Only dependency: `chrono` (alloc feature) for the date formatting
(`Cargo.toml:12`).
