# launchk

`packages/launchk` builds [launchk](https://github.com/mach-kernel/launchk), a
cursive (Rust TUI) tool for observing launchd agents and daemons, from source.
It is the reference shape for the external-Rust-tool house style
(`agent-context/sections/13-dependency-intake.md`, "Packaging external Rust
CLI/TUI tools"), and is macOS-only because launchk talks to launchd over XPC.

## What this repo changes

`default.nix` builds the upstream crate with
`rustPlatform.buildRustPackage` from a pinned source, with two build fixups
(`packages/launchk/default.nix`):

- Source pin: `ix.launchkSrc`, the `launchk-src` flake input
  (`flake = false`, `github:mach-kernel/launchk/6f5f09e...`,
  `flake.nix:105-108`), threaded into the `ix` bundle as `launchkSrc`
  (`lib/default.nix:431`).
- Version: `"0.3.1-unstable-2025-06-07"`, the nixpkgs unstable-version spelling
  because there is no upstream release tag past the crate version, so a rev bump
  reads as a dated change (`packages/launchk/default.nix:14-16`).
- Lockfile: `cargoLock.lockFile = src + "/Cargo.lock"` reads upstream's
  committed pure-crates.io lock, so a rev bump carries the dependency set with
  no checked-in lock to drift and no coarse `cargoHash` to refresh
  (`packages/launchk/default.nix:20-23`).
- Fixup 1 (bindgen): `xpc-sys` generates XPC framework bindings with bindgen,
  which needs libclang, so `rustPlatform.bindgenHook` is in `nativeBuildInputs`
  (`packages/launchk/default.nix:27-29`).
- Fixup 2 (`git_version!()`): the about-box string shells out to `git describe`
  at build time, but the fetched tarball has no `.git`; `postPatch` rewrites the
  macro to `env!("CARGO_PKG_VERSION")` with `--replace-fail` (which errors if
  upstream moves the call, keeping the patch honest)
  (`packages/launchk/default.nix:31-37`).
- Build scope: `cargoBuildFlags`/`cargoTestFlags` restrict to `-p launchk`
  (`packages/launchk/default.nix:39-46`).
- `meta`: MIT, `mainProgram = "launchk"`, `platforms = darwin`
  (`packages/launchk/default.nix:48-54`).

## Build and wiring

- Flake output: `nix run .#launchk` / `nix build .#launchk`, gated to Darwin.
  `package.nix` sets both `packageSet.systems` and `flake.systems` to
  `aarch64-darwin` + `x86_64-darwin` (`packages/launchk/package.nix:7-14`) so
  `nix flake check` does not force nixpkgs to evaluate this Linux-impossible
  package off-platform.
- Platform constraint: launchd/XPC is macOS-only; the `meta.platforms` gate and
  the `package.nix` systems gate are kept in sync per the intake policy.
- Bump: `nix flake update launchk-src` (or repoint the rev in `flake.nix`) and
  update the dated `version` string. No `manifest.json`/`updateScript`.
