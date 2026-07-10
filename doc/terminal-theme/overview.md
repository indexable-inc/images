# terminal-theme

`packages/terminal-theme` owns one decision shared across the repo's terminal
tools: is the terminal background light or dark. It is a tiny Rust library crate
(`Cargo.toml:2`), a workspace member with no flake output, consumed by
`packages/search` and `packages/git-log-pretty` (`Cargo.toml:30` and
`:26` respectively) to pick a palette.

## Public surface (`src/lib.rs`)

- `Theme::{Light, Dark}` (`src/lib.rs:20`), `Dark` being the default and the
  fallback (`#[default]`, `:24`).
- `detect() -> Theme` (`src/lib.rs:35`): probe the terminal and classify.

The whole crate is two enum variants and one function.

## Behavior and the TTY gate

`detect` probes the background luma via the
[`terminal-light`](https://docs.rs/terminal-light) crate and classifies anything
brighter than mid-gray (luma > 0.5) as `Light`, everything else (including an
unreadable or absent response) as `Dark` (`src/lib.rs:39-42`).

The load-bearing invariant is the TTY gate: the probe works by writing an OSC
color query and waiting for the terminal to answer, so it only runs when stdout
is a terminal (`std::io::stdout().is_terminal()`, `src/lib.rs:36`). Under a pipe,
a capture, or a test, those query bytes would corrupt the output or block on a
reply that never comes, so non-interactive stdout returns `Theme::Dark` without
probing. A caller with its own "should I emit color at all" switch (a `--color`
flag) keeps that decision local and only calls `detect` once it has decided
color is wanted (`src/lib.rs:11-13`).

## Build

`package.nix` is `{ id; inRustWorkspace = true; passthruTests = true; }`: no
`default.nix`, no flake output, built only as a workspace library. Its single
dependency is `terminal-light` (`Cargo.toml:12`). Unit tests assert the default
is `Dark` and that the test harness's captured (non-TTY) stdout returns the dark
fallback (`src/lib.rs:49-59`).
