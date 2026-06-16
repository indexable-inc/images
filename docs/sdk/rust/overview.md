# Rust SDK

`sdk/rust` is a two-crate Cargo workspace (`sdk/rust/Cargo.toml:15-20`) that
demonstrates and exports the ix SDK wire-protocol layer. The public crate
`ix-sdk` re-exports the boundary types from `ix-sdk-wire`, and the Nix build
links the real `ix-sdk-wire` rlib fetched from R2 over a metadata-faithful
source stub, proving the consumer can typecheck and link the prebuilt artifact
with no private source in its closure. This is the wire layer, not a full HTTP
client: the `Client`/`Branch` surface lives only in the Python and TypeScript
packages.

See [common](../common.md) for the cross-SDK model and licensing.

## Member crates

| crate | path | role |
| --- | --- | --- |
| `ix-sdk` | `sdk/rust/crates/ix-sdk` | public crate; re-exports the wire surface and a `normalize_error_code` helper; also builds the `ix-sdk-wire-probe` binary |
| `ix-sdk-wire` | `sdk/rust/vendor/ix-sdk-wire` | metadata-faithful STUB of the private wire crate; exists only to give Cargo a dep edge and to reproduce the prebuilt's unit hash |

Both are `version = "0.1.0"`, `edition = "2024"`, `publish = false`, governed by
`sdk/LICENSE` via `license-file` (`sdk/rust/crates/ix-sdk/Cargo.toml:1-11`,
`sdk/rust/vendor/ix-sdk-wire/Cargo.toml:23-35`).

## Public surface (`ix-sdk`)

`crates/ix-sdk/src/lib.rs` re-exports the wire types so consumers reach them
through `ix_sdk::*` (`src/lib.rs:12-14`):

- `IxBuf`, `IX_BUF_VERSION` - the buffer type and version constant crossing the
  C-ABI boundary.
- `Encoder`, `Decoder`, `DecodeError` - serialization of wire messages.
- `IxError`, `IxErrorCode`, `IxErrorKind` - the structured error surface; error
  codes are an append-only roster (`strum::VariantArray`-backed) with `0` as the
  reserved `Unknown` sentinel.

`normalize_error_code(raw: u32) -> u32` (`src/lib.rs:21-24`) round-trips a code
through `IxErrorCode::from_u32(raw).as_u32()`. The `use` itself is the compile
gate: those symbols must exist in the injected rlib for the crate to link
(`src/lib.rs:9-14`).

The `ix-sdk-wire-probe` binary (`[[bin]]`,
`crates/ix-sdk/Cargo.toml:15-20`, `src/bin/probe.rs`) calls
`normalize_error_code(0)` and `normalize_error_code(u32::MAX)`, both of which
fold to `0`, and prints `ix-sdk-wire linked: normalize(0)=0 normalize(MAX)=0`.
That string is what the Nix proof greps for (`default.nix:236`) to prove the
prebuilt rlib was linked and run, not merely typechecked.

## The wire stub (`ix-sdk-wire`)

`vendor/ix-sdk-wire` is NOT the real wire crate; its `src/lib.rs` is a trivial,
never-compiled body (`vendor/ix-sdk-wire/src/lib.rs:1-8`). What matters is its
Cargo metadata, which must match the real crate exactly because cargo-unit
folds package identity (name + version), edition, crate-types, features,
resolved dependency identities, profile, lints, and toolchain id into a
source-independent unit hash, never the source bytes
(`vendor/ix-sdk-wire/Cargo.toml:8-22`). The dependencies are pinned to mirror
ix: `snafu` from the shepmaster git fork (bare URL, rev pinned in `Cargo.lock`)
and `strum = "0.28"` with `derive` (`Cargo.toml:39-47`). `[lints] workspace =
true` inherits the workspace lint table so `lint_rustflags` (hence the hash)
matches (`Cargo.toml:49-50`).

## Profiles and lints (hash inputs)

The workspace `Cargo.toml` reproduces ix's build profile byte-for-byte because
the profile is a hash input. `[profile.release]` turns on `debug-assertions`
and `overflow-checks` (`Cargo.toml:29-31`); `[profile.release.package."*"]`
turns them off for third-party crates (`Cargo.toml:38-40`); and
`[profile.public-rlib]` is the exact profile the R2 rlib was compiled under:
`codegen-units=1`, `lto="off"`, `opt-level="z"`, `panic="unwind"`, `strip=true`,
no debuginfo, plus `-Zlocation-detail=none` and a `/nix/store=/source` path
remap (`Cargo.toml:50-60`). `cargo-features = ["profile-rustflags"]` opts into
the unstable per-profile `rustflags` (`Cargo.toml:4`). `[workspace.lints.rust]`
mirrors ix's deny/allow/warn table (`Cargo.toml:69-86`).

## How it is built and wired (`sdk/rust/default.nix`)

The build links the prebuilt `ix-sdk-wire` rlib WITHOUT its source
(`default.nix:1-16`):

1. **Fetch from R2 by SRI** (`default.nix:69-76`): `pkgs.fetchurl` pulls
   `libix_sdk_wire-<hash>.rlib` and `.rmeta` from
   `r2.dev/rlib/ix-sdk-wire/<wireHash>`; the SRI hash is the store identity, so
   the URL carries no secret. `wireHash = "a95096d6b0ee69a6"` is the prebuilt's
   source-independent unit hash (`default.nix:62`).
2. **Wrap as a library unit** (`default.nix:83-93`):
   `cargoUnit.mkPrebuiltLibraryUnit` records the rlib, rmeta, and the
   `wireToolchainId`
   (`iz0mdcq43pxl3fmxmznc6n38sals6q0x-rust-default-1.98.0-nightly-2026-05-27`,
   `default.nix:63`). An eval-time assert rejects a wrong toolchain before link.
3. **Inject over the stub** (`default.nix:174-184`):
   `cargoUnit.buildWorkspace` is called with `extraUnits` / `extraLibraries`
   keyed by `wireUnitKey = "ix_sdk_wire-${wireVersion}-${wireHash}"`
   (`default.nix:98`). The stub's generated key must equal this, or
   `buildWorkspace`'s C1 assert fires.

To make the generated stub hash equal the prebuilt's, the workspace builds with
the same toolchain (`ix.languages.rust.toolchain`, `default.nix:39-54`), the
same `--target` (`hostRustTarget`, `default.nix:120-127`, `144`), the same
`profile = "public-rlib"` (`default.nix:152`), and the same snafu git tree SRI
(`outputHashes`, `default.nix:134-137`).

### The proof derivation

`proof` (`default.nix:199-258`) is the end-to-end check, built on the fleet. It
asserts: (a) the injected unit drv differs from the from-source stub unit drv
and the unit map resolves to the prebuilt (`default.nix:211-223`); (b) the
prebuilt unit's `$out` holds the rlib, rmeta, and `nix-support/extern-path`
(`default.nix:225-229`); (c) running `ix-sdk-wire-probe` prints the expected
linked-rlib line (`default.nix:231-236`); (d) via `exportReferencesGraph`, the
from-source stub unit drv is absent from the consumer's build closure while the
prebuilt unit is present (`default.nix:238-254`).

### Flake wiring

`sdk/rust` has no `package.nix`, so it is not a `nix run .#<name>` flake output.
It is imported directly by the eval tests
(`tests/default.nix:21`, `sdkRust = import ../sdk/rust { ... }`) and exposed as
the check `sdkRustPrebuilt = sdkRust.proof` (`tests/default.nix:5091`), which
gates the prebuilt-link contract in CI.
