# snix

`packages/snix` builds the snix `default` CLI (a Rust reimplementation of Nix,
TVL depot `git.snix.dev/snix/snix`) through this repo's
[nix-cargo-unit](../nix-cargo-unit/overview.md) engine instead of snix's own
crate2nix packaging. It is a Nix-only package: the source is an external,
fetched Cargo workspace, not a member of this repo's workspace.

## Purpose

snix upstream packages itself with crate2nix (`snix/Cargo.nix` +
`crate-hashes.json`): roughly 1100 one-derivation-per-crate builds plus feature
powersets, with no shared incrementality (`default.nix:11-17`). This package
builds the same CLI through `ix.cargoUnit.buildWorkspace`, so snix compiles as
one Nix derivation per Cargo rustc unit with source-scoped, content-addressed
inputs: the same engine that builds the rest of the repo's Rust tree. It is also
the worked example of `cargoUnit` building an arbitrary external workspace, not
just this repo's.

## How it builds (`default.nix`)

- **Source**: the `snix-src` flake input, surfaced as `ix.snixSrc`; the Cargo
  workspace lives in its `snix/` subdirectory (`default.nix:19-22`).
- **`ix.cargoUnit.buildWorkspace`** (`default.nix:24`) with `pname = "snix"`,
  `src`/`workspaceRoot` both the `snix/` dir, `cargoLock` its `Cargo.lock`, and
  `cargoArgs = [ "--workspace" ]`.
- **Relaxed third-party policy** (`default.nix:36-41`): build it, do not lint or
  audit it: `denyUnusedCrateDependencies`, `cargoAudit`, `cargoMachete`, and
  `clippy` all disabled (snix is not this repo's code to gate).
- **Build-script tooling** applied to every unit (`default.nix:49-64`):
  `protobuf` (prost/tonic `protoc`), `pkg-config` (`*-sys` crates), `cmake`+`perl`
  (aws-lc-sys, rustls' default backend). `PROTOC`/`PROTOC_INCLUDE` are set, and
  `PROTO_ROOT` points at the whole snix checkout because cargo-unit gives each
  build script a per-crate scoped `CARGO_MANIFEST_DIR`, which snix's `.proto`
  resolution would otherwise break on.
- **Git dependency hashes** pinned by exact lock source string in `outputHashes`
  (`default.nix:69-76`); refresh with `nix flake update snix-src`.

## The `default` CLI assembly

snix ships a base `snix` dispatcher (crate `snix-cli`, bin `snix`) that finds each
`snix-<subcommand>` binary on `SNIX_LIBEXEC_PATH`, mirroring snix's own
`cli/make-cli.nix` + `cli/default-cli.nix` (`default.nix:79-83`). The package
`runCommand` symlinks the eight subcommand binaries from
`workspace.binaries.<name>` into `$out/libexec` and `makeWrapper`s the `snix`
binary with `SNIX_LIBEXEC_PATH` suffixed to that dir (`default.nix:95-114`):

`snix-build`, `snix-castore`, `snix-castore-http`, `snix-derivation-show`,
`snix-eval`, `snix-nar-bridge`, `snix-nix-daemon`, `snix-store`
(`default.nix:84-93`). virtiofs is a Linux-only non-default feature, so the plain
`--workspace` graph omits it and the binary set is identical across platforms.

## Packaging

`package.nix`: `flake = true`, `packageSet = true`; NOT `inRustWorkspace` (it is
an external fetched workspace). `meta`: GPL-3.0-only, `mainProgram = "snix"`,
`platforms = unix`. Flake output / package-set name: `snix`. `passthru.workspace`
exposes the full `buildWorkspace` result. Run as `nix run .#snix`.
