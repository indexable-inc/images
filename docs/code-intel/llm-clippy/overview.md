# llm-clippy

`packages/llm-clippy` builds a fork of `rust-lang/rust-clippy` carrying extra
restriction lints tuned for LLM-assisted codebases. It is the Clippy the rest of
the repo's Rust packages are checked with: the cargo-unit policy wires this
binary in as every package's Clippy gate (`lib/rust/tooling.nix:26`).

## What is special: it is Nix-only

Unlike every other package in this domain, `llm-clippy` is **not** a Rust
workspace member: it is in the `exclude`/absent set of the root
[`Cargo.toml`](../../../Cargo.toml) and its `package.nix` sets neither
`inRustWorkspace`. It builds an external source tree, not workspace code.

- The source is the `clippy-fork` flake input
  (`github:indexable-inc/clippy`, `flake.nix:85-88`), consumed as a `flake =
  false` source tree. `nix flake update clippy-fork` bumps it.
- `default.nix` requires `clippy-fork` (it throws otherwise, taking the fork as
  `clippy-fork`, never `src`, to avoid colliding with `pkgs.src`) and builds it
  with `ix.buildRustPackage`. The toolchain is read from the fork's own
  `rust-toolchain.toml` (`pkgs.rust-bin.fromRustupToolchainFile`), so a fork bump
  advances the `rustc`/`rustc_private` ABI in lockstep. `cargoLock.lockFile`
  reads the fork's lockfile (no checked-in lock to drift).
- `env.RUSTC_BOOTSTRAP = "1"` because the fork links `rustc_private` crates;
  `postInstall` wraps `cargo-clippy` and `clippy-driver` with the toolchain's lib
  dir on `LD_LIBRARY_PATH` (or `DYLD_LIBRARY_PATH` on Darwin).
- Its own build disables the recursive policy checks: `policy.clippy.enable =
  false` (it is the Clippy others use; checking itself would recurse through
  `llmClippyFor`) and `policy.cargoMachete.enable = false` (the fork ships UI
  fixtures that are not build workspaces).

## Outputs

- Flake output `llm-clippy` (`package.nix`: `id = llm-clippy`, `flake = true`,
  `packageSet = true`); `meta.mainProgram = clippy-driver`. `nix build
  .#llm-clippy`.
- Binaries `cargo-clippy` and `clippy-driver`; `passthru.toolchain` exposes the
  pinned toolchain.

## The extra lints

The fork adds restriction lints the workspace then denies in the root
`Cargo.toml` lint table (`Cargo.toml:121-130`):

- `anonymous_tuple_return_type`: forbid an anonymous tuple return type, forcing a
  named struct so a multi-value return is readable at the call site (used e.g. in
  `packages/blast-radius/src/main.rs:110`).
- `fallible_int_fallback`: forbid silently defaulting a fallible integer
  conversion (`u8::try_from(x).unwrap_or(...)` / `.unwrap_or_default()` /
  `.unwrap_or_else(...)`), which would clamp or zero an out-of-range value and
  hide the overflow; propagate the `TryFromIntError` instead.

Both are off even in Clippy's `all` group and only exist because this fork
provides them, which is why the workspace must build against `llm-clippy` rather
than stock Clippy.

## Wiring (`lib/rust/tooling.nix`)

`rustFor` calls `pkgs.callPackage (packagePath "llm-clippy")` with the
`clippy-fork` input, then hands the resulting Clippy package to the Rust build
(`lib/rust/build.nix`). `buildRustPackage` attaches the `llm-clippy` check as a
policy dependency of every repo Rust package (`tooling.nix:81-88`), so a lint
the fork adds fails the affected package's build, not just a separate gate.
`llm-clippy` bootstraps before `cargoUnit`/`rustWorkspace` exist, so it receives
only `buildRustPackage` from the `ix` closure (`tooling.nix:23-29`).
