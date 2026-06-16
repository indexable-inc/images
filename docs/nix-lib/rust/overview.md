# lib/rust: cargo-unit Rust build

`lib/rust/` is the core builder: it compiles a Rust workspace as one Nix
derivation per Cargo rustc unit, so a source edit in `crates/api` does not
rehash the input for `crates/worker` or any vendored crate. It also offers a
single-derivation `buildPackage` path (`rustPlatform.buildRustPackage` wrapped
with the repo policy gates) for standalone crates. The heavy lifting (unit-graph
parse, render, hashing) lives in the [nix-cargo-unit](../../nix-build/nix-cargo-unit/overview.md)
Rust CLI; this directory is the Nix front end that drives it.

## Files

| file | role |
| --- | --- |
| `lib/rust/cargo-unit.nix` | `buildWorkspace` and the binary/library selectors; the per-unit build front end |
| `lib/rust/build.nix` | `buildPackage`: single-derivation `buildRustPackage` + policy |
| `lib/rust/resolve.nix` | resolution boundary: raw args -> one `{ context, policy, effects, checks }` bundle, applied once |
| `lib/rust/vendor.nix` | `Cargo.lock` -> package-shaped vendor dir + cargo config script |
| `lib/rust/policy.nix` | the quality-gate schema and check derivations (clippy, audit, machete, ...) |
| `lib/rust/workspace.nix` | the shared repo-wide unit graph (`rustWorkspaceFor`) |
| `lib/rust/tooling.nix` | bootstrap: `buildIxRustTool`, `cargoUnitFor`, `buildRustPackage` factories |

## How it is wired

`lib/rust/tooling.nix` is the bootstrap seam. `rustFor pkgs`
(`lib/rust/tooling.nix:19-35`) imports `build.nix` with the repo's pinned
nightly toolchain (read from `rust-toolchain.toml`, `lib/rust/tooling.nix:13-18`)
and the `llm-clippy` package. `cargoUnitFor pkgs`
(`lib/rust/tooling.nix:74-80`) imports `cargo-unit.nix` with
`rust = rustFor pkgs` and `nixCargoUnit = buildIxRustTool pkgs (packagePath
"nix-cargo-unit")`. `buildIxRustTool` (`lib/rust/tooling.nix:40-72`) builds a
repo Rust tool while keeping `nix-cargo-unit` itself on the pre-cargo-unit path
(it is what builds the graph), and prefers the policy-unchecked variant so a
generator does not drag the check graph into its closure.

`lib/default.nix` exposes `cargoUnit = cargoUnitFor pkgs`
(`lib/default.nix:163`), `buildRustPackage` (in `ixSpecialArgs`,
`lib/default.nix:441`), and `cargoUnitFor`/`buildRustPackage` factories in
`ixReturn`. Most repo crates do not call these directly; they select out of the
shared workspace (below).

## resolve.nix: the resolution boundary

`resolveArgs args` (`lib/rust/resolve.nix:59-147`) is where every multiply-read
default/transform happens once. It returns:

- `context`: the "run cargo in the vendored tree" bundle: `src`,
  `rustToolchain` (default from `rust-toolchain.toml`), `vendorDir`,
  `vendorSources`, `env`, `nativeBuildInputs`, plus `cargoLockPath`,
  `toolchainId`, and `configScript` resolved once (`lib/rust/resolve.nix:80-93`).
- `policy`: the resolved gate decisions (typed schema, below).
- `effects`: policy consequences computed once: rustc args (mold), linker native
  inputs, clippy lint flags, renderer deny-flags (`lib/rust/resolve.nix:95-103`).
- `checks`: the two altitude-appropriate check sets bound to this context:
  `crate` (audit+machete+clippy) and `workspace` (audit+machete; per-unit clippy
  runs in the renderer) (`lib/rust/resolve.nix:120-136`).

The **toolchain id** is the basename of the toolchain store path
(`lib/rust/resolve.nix:37-41`); it is baked into every unit hash, so it is
derived here once rather than re-spelled at each use.

## vendor.nix: vendoring

`mkVendor { cargoLock, outputHashes, sourceOverrides }`
(`lib/rust/vendor.nix:146-313`) reads `Cargo.lock` and produces, lazily:

- `vendorSources`: one package-shaped directory per dependency. Registry crates
  are fetched from `static.crates.io` (the direct CDN; the legacy
  `api.crates.io` endpoint now 403s curl, `lib/rust/vendor.nix:49-61`) and
  verified against the lockfile checksum; git crates are `fetchgit`'d, reduced to
  the referenced package, and have workspace inheritance flattened by the
  vendored `replace-workspace-values.py` (`lib/rust/vendor.nix:216-272`).
- `vendorDir`: an aggregate `linkFarm` symlinking each entry under
  `<name>-<version>` (`lib/rust/vendor.nix:290-309`).

Git deps require `outputHashes."git+<url>#<rev>" = "sha256-..."` keyed by the
exact `Cargo.lock` source string; a missing or extra key throws
(`lib/rust/vendor.nix:155-175`). `vendorConfigScript`
(`lib/rust/vendor.nix:78-138`) emits the `$CARGO_HOME/config.toml` that points
cargo at the vendor dir plus a `[source."<git>"]` replace block per git dep.

## policy.nix: the quality gates

The schema is declared once as NixOS module options
(`lib/rust/policy.nix:35-143`) so defaults, the caller-merge, and typo rejection
(no freeform type) all come from `resolvePolicy` via `evalModules`
(`lib/rust/policy.nix:164-185`). Knobs and defaults:

- `denyUnusedCrateDependencies` (true): rustc gate on unused declared deps.
- `denyPanics` (false): best-effort panic-reachability scan.
- `cargoAudit.enable` (true): offline, lockfile-only `cargo-audit` against a
  pinned advisory-db (`lib/rust/policy.nix:49-78`, `279-308`).
- `cargoMachete.enable` (true): unused-dep scan (disabled for the repo workspace
  in favor of the per-crate rustc gate, `lib/rust/workspace.nix:264-268`).
- `clippy.enable` (true): per unit in a workspace, whole-crate otherwise; lints
  default to denying warnings (`lib/rust/policy.nix:91-122`).
- `tests.enable`/`useNextest` (true), `linker.useMold` (Linux).

`clippyLintFlagsFromManifest` (`lib/rust/policy.nix:208-261`) parses
`[lints.clippy]` from the workspace `Cargo.toml` into `-D|-W|-A` flags, because
cargo only injects those into the unit graph under `cargo clippy`, not
`cargo build`. `policyPresets.pureBuild` turns every gate off
(`lib/rust/policy.nix:150-157`) for pure artifacts (cross graphs, prebuilt
injection) where another graph already ran the gates.

## cargo-unit.nix: buildWorkspace

`buildWorkspace rawArgs` (`lib/rust/cargo-unit.nix:121-505`) is the entry point.
Two IFD stages:

1. `unitGraphJson` (`lib/rust/cargo-unit.nix:211-287`): runs
   `cargo build --unit-graph` once per `cargoTargets` entry (in parallel) in the
   vendored tree, merging them with `nix-cargo-unit merge`.
2. `unitsNix` (`lib/rust/cargo-unit.nix:299-319`): renders `units.nix` from the
   graph via `nix-cargo-unit render`, passing `--workspace-root`,
   `--vendor-root`, `--toolchain-id`, `--cargo-lock`, and `--content-addressed`
   (default true, `lib/rust/cargo-unit.nix:301`).

Importing `units.nix` (`importUnits`, `lib/rust/cargo-unit.nix:333-386`) yields
the per-unit derivations and threads the workspace `env`, `extraRustcArgs`,
per-package test/build env, and the resolved clippy/policy flags. The returned
attrset carries `units`, `binaries`, `libraries`, `benchmarks`, `tests`,
`doctests`, `targetSets.<name>`, `coverageReport`, plus the intermediate
`unitGraphJson`/`unitsNix`/`vendorDir` for inspection
(`lib/rust/cargo-unit.nix:108-111`, `500-505`).

Key arguments: `workspaceRoot` (required, the real checkout root scopes are
carved from, `lib/rust/cargo-unit.nix:140-145`); `src` (the filtered build
input); `cargoTargets`/`cargoTargetNames` (several Cargo executions through one
graph, e.g. `[["--workspace"] ["--workspace" "--tests"]]`); `profile`/`target`;
`policy`; `env` (folds into every unit, so use `packageBuildEnv.<pkg>` for a
value that changes often, `lib/rust/cargo-unit.nix:79-84`); `extraRustcArgs` and
`extraRustcArgsForPlatform`; `cargoConfigRustflags` (apply
`.cargo/config.toml` rustflags). Selecting `binaries.<n>`, `libraries.<n>`, or a
`targetSets.<set>` entry builds only that root's unit closure
(`lib/rust/cargo-unit.nix:74-91`).

### Selectors

- `buildBinary { binary, ... }` / `buildBinaries { binaries, ... }`: build a
  fresh workspace and pick a root (`lib/rust/cargo-unit.nix:519-524`, `670-675`).
- `selectBinaryWithTests workspace { binary, packageName ? binary, ... }`
  (`lib/rust/cargo-unit.nix:539-557`) and `selectLibraryWithTests`
  (`lib/rust/cargo-unit.nix:568-586`): pick a root out of a **shared** workspace
  plus its test/doctest/policy derivations as `passthru.tests`. This is the
  common path: a crate's `default.nix` is often just
  `ix.cargoUnit.selectBinaryWithTests ix.rustWorkspace.units { binary = "tap"; }`
  (`packages/tap/default.nix`).

### Prebuilt unit injection

`mkPrebuiltLibraryUnit { name, version, hash, rlib, rmeta, toolchainId, depUnits ? [] }`
(`lib/rust/cargo-unit.nix:749-819`) builds a library unit from already-compiled
artifacts, byte-contract-identical to what the renderer emits, keyed by
`<name>-<version>-<hash>`. Pass it through `buildWorkspace`'s `extraUnits` to
link a downstream crate against a prebuilt rlib with no source present
(rlib-only; bypasses per-unit policy gates, so inject only trusted artifacts).
Four eval-time guards (C1-C4, `lib/rust/cargo-unit.nix:397-474`) reject an
injection key absent from the graph, a toolchain-id mismatch, a key disagreeing
with the unit's recorded `unitKey`, or two prebuilts claiming one key.

## workspace.nix: the shared repo unit graph

`rustWorkspaceFor pkgs` (`lib/rust/workspace.nix:16`) returns
`{ root, src, cargoLock, units, dashboardSite, unitsFor }`. `units` is one
`buildWorkspace` over every `inRustWorkspace` registry crate
(`lib/rust/workspace.nix:124-285`), with `cargoTargets` covering build, test, and
bench root sets (`lib/rust/workspace.nix:171-190`) and the policy that gates the
whole repo (`lib/rust/workspace.nix:257-270`). It also wires the non-obvious
native-link plumbing: `IX_VT_GHOSTTY_LIB_DIR` + a workspace `-L`/rpath for
libghostty-vt, `IX_DASHBOARD_SITE_HTML`, ALSA pkg-config for the minecraft sound
crate, and host-gated libkrun for vmkit (`lib/rust/workspace.nix:191-252`).
`unitsFor { target }` (`lib/rust/workspace.nix:308-312`) builds a cross graph
(Apple targets go through the [darwin](../darwin/overview.md) zig+SDK toolchain);
it skips tests/benches and policy, building only the `--workspace` root set.

`lib/default.nix` binds `rustWorkspace = rustWorkspaceFor pkgs`
(`lib/default.nix:366`); both `rustWorkspace` and `rustWorkspaceFor` ride
`ixSpecialArgs`/`ixReturn`.

## build.nix: single-derivation buildPackage

`buildPackage` (`lib/rust/build.nix:32-163`) wraps
`rustPlatform.buildRustPackage` for a standalone crate. `srcRoot = ./.` is the
shortcut for a repo crate (gitTracked filter + `meta.mainProgram` default,
`lib/rust/build.nix:43-58`). It vendors through the repo's own `vendorDir`
(faster than nixpkgs `importCargoLock`, `lib/rust/build.nix:81-99`), enables
nextest, and returns a `symlinkJoin` carrying the policy checks as
`passthru.tests` and under `$out/rust-policy` with the unchecked package at
`passthru.unchecked` (`lib/rust/build.nix:140-163`). Reached as
`ix.buildRustPackage pkgs` and used by tools that need their own workspace
(different policy, fetched source) rather than the shared graph.
