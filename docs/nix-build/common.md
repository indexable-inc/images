# nix-build

Nix build-system integration for the `index` repo: the tooling that turns the
Cargo workspace into a per-unit Nix derivation graph, watches and explains those
builds, packages their outputs as OCI images, and supplies the small CLI helpers
every ix tool shares. The center of gravity is [nix-cargo-unit](nix-cargo-unit/overview.md),
the bootstrap renderer that the [nix-lib](../nix-lib/common.md) domain
(`lib/rust/cargo-unit.nix`) wraps into `ix.cargoUnit`; every Rust package here is
then built one rustc invocation at a time as its own derivation. The rest of the
domain is observability and packaging around that engine: terminal/web build
monitors, a CA-aware `nom`, an OCI image builder, a PR rebuild-impact reporter,
the package registry that drives the flake outputs, an out-of-tree Nix
reimplementation built through the same engine, and two reusable runtime crates.

Read this page first, then the component page for the unit you are touching.

## Units

| unit | kind | role |
| --- | --- | --- |
| `packages/nix-cargo-unit` | standalone Cargo workspace, Nix package + flake output `nix-cargo-unit` | render a Cargo `--unit-graph` into composable Nix derivations; merge graphs; scan rlibs for panic reachability. See [nix-cargo-unit](nix-cargo-unit/overview.md). |
| `packages/nix-web-monitor` | Rust workspace crates (`parser`, `server`), flake output `nix-web-monitor` | run a Nix build with quiet terminal output and a live web monitor; the parser models the `internal-json` event stream. See [nix-web-monitor](nix-web-monitor/overview.md). |
| `packages/nix-output-monitor` | Nix-only (Haskell override), flake output `nix-output-monitor` (`nom`) | upstream nix-output-monitor patched so `nix-derivation` parses content-addressed derivations. See [nix-output-monitor](nix-output-monitor/overview.md). |
| `packages/oci-image-builder` | Rust workspace crate, flake output + overlay `oci-image-builder` | turn a `streamLayeredImage` layer plan into an OCI tar, or describe/materialize it through a content-addressed `image.json`. See [oci-image-builder](oci-image-builder/overview.md). |
| `packages/blast-radius` | Rust workspace crate, flake output `blast-radius` | report how many `.#checks.x86_64-linux` derivations a PR rebuilds and which changed inputs caused each rebuild. See [blast-radius](blast-radius/overview.md). |
| `packages/registry.nix` | Nix-only library (not a flake output) | discover every `packages/**` unit's `package.nix` metadata and drive the flake/overlay/packageSet/test lists. See [registry.nix](registry/overview.md). |
| `packages/snix` | Nix-only (external Cargo workspace built via cargo-unit), flake output `snix` | the snix `default` CLI (a Rust reimplementation of Nix) built through `ix.cargoUnit`. See [snix](snix/overview.md). |
| `packages/build-version` | Rust workspace crate (library) | format a binary's `--version` line from Nix-stamped build metadata. See [build-version](build-version/overview.md). |
| `packages/config-launch` | Rust workspace crate, flake output `config-launch` | spec-driven exec launcher: set env/PATH and inject CLI flags, then `exec` the target preserving argv0. See [config-launch](config-launch/overview.md). |
| `packages/progress-style` | Rust workspace crate (library) | shared `indicatif` progress-bar/spinner styling for ix CLIs. See [progress-style](progress-style/overview.md). |

Rust workspace members (root `Cargo.toml`): `nix-web-monitor/parser`,
`nix-web-monitor/server`, `oci-image-builder`, `blast-radius`, `build-version`,
`config-launch`, `progress-style`. Standalone/Nix-only: `nix-cargo-unit`
(excluded from the root workspace, carries its own `Cargo.lock`),
`nix-output-monitor` (Haskell), `registry.nix` (Nix), `snix` (external Cargo
workspace fetched as a flake input).

## How it fits together

```
Cargo.toml workspace
   cargo +toolchain build --unit-graph -Z unstable-options   (per cargoTargets entry)
      -> unit-graph JSON  --(nix-cargo-unit merge)-->  one merged graph
         --(nix-cargo-unit render --cargo-lock ...)-->  units.nix
            ix.cargoUnit.buildWorkspace imports units.nix:
               one stdenv.mkDerivation per rustc unit (lib/bin/test/doc/build-script)
               source-scoped per crate; optional --content-addressed; clippy/panic/audit checks
   ix.cargoUnit.selectBinaryWithTests -> packages/<tool>/default.nix flake outputs
                                         (oci-image-builder, blast-radius, config-launch, nix-web-monitor, snix)

build observability:
   nix build --log-format internal-json
      -> nix-web-monitor/parser  (events -> MonitorState/Delta -> web UI + JSON)
      -> nix-output-monitor (nom)  (terminal DAG; CA-derivation aware)
   blast-radius  diffs .#checks at base vs head over the same per-unit graph

packaging / runtime:
   dockerTools.streamLayeredImage conf.json -> oci-image-builder -> OCI tar / image.json (lib/image)
   build-version    renders --version from IX_BUILD_REV / IX_BUILD_EPOCH the Nix wrapper stamps
   progress-style   the shared indicatif styles ix CLIs draw with

discovery:
   packages/registry.nix reads every package.nix -> flake/overlay/packageSet/passthruTests lists (lib/per-system.nix)
```

- **One derivation per rustc unit.** A Cargo unit graph node (one crate compiled
  in one mode with one feature set) becomes one Nix derivation, identity-hashed
  from its package, target, profile, features, flags, and the hashes of its
  dependency units (`packages/nix-cargo-unit/src/model.rs:612`,
  `:638`). Editing one crate moves only that unit's hash and its dependents',
  so a build reuses the rest of the store. This is the shared substrate every
  Rust tool in this domain (and most of the repo) is built on.

- **nix-cargo-unit bootstraps itself.** It renders the rest of the workspace's
  unit graph, so it cannot be built through `ix.cargoUnit`. It is a standalone
  Cargo workspace built as a plain `ix.buildRustPackage`
  (`packages/nix-cargo-unit/default.nix:16`), excluded from the root workspace
  (`Cargo.toml` `exclude`) so a root-lock churn never recompiles it.

- **Build identity is input-addressed by basename.** Because each unit is a
  derivation whose `.drv` basename is `<hash>-<name>.drv` and the hash folds in
  every input, an identical basename across two revisions means an identical
  build. [blast-radius](blast-radius/overview.md) and the CA-derivation handling
  in the monitors both rely on this: a moved basename is a real rebuild.

- **The monitors read Nix's `internal-json`, not the build graph.** That stream
  names each derivation Nix builds but carries no edges and no per-activity
  success marker. [nix-web-monitor](nix-web-monitor/overview.md) reconstructs the
  DAG out-of-band with `nix-store --query --requisites`, taps the daemon's
  syscalls for the otherwise-invisible `addToStore` phase, and folds
  content-addressed `resolved derivation` pairs into one row;
  [nom](nix-output-monitor/overview.md) needs a patched `nix-derivation` to parse
  the empty output path a floating CA derivation carries.

- **Nix stamps the build, runtime renders it.** A Nix wrapper sets `IX_BUILD_REV`
  and `IX_BUILD_EPOCH` (mirroring `ix.rev` / `ix.revEpoch`) in the environment;
  [build-version](build-version/overview.md) reads them at startup to format the
  `--version` line, so a new commit re-stamps a tiny wrapper instead of
  rebuilding the Rust unit. nix-web-monitor's wrapper is the worked example
  (`packages/nix-web-monitor/server/default.nix:59`).

## Invariants

- **Fail closed on a missing or unreadable input.** nix-cargo-unit's panic gate
  errors when it finds no artifacts to scan rather than passing
  (`packages/nix-cargo-unit/src/main.rs:144`), and treats an unparseable
  artifact as an error (`src/panic_scan.rs:107`). blast-radius bails when a check
  newly fails to evaluate at head (`packages/blast-radius/src/main.rs:81`) and
  when a `nix-eval-jobs` row has neither `drvPath` nor `error`
  (`src/nix.rs:194`). The web monitor propagates Nix's exit code instead of
  masking it (`packages/nix-web-monitor/server/src/main.rs:160`).

- **Best-effort overlays never fail the run.** The daemon syscall tracer degrades
  to a status string on any error rather than aborting
  (`packages/nix-web-monitor/server/src/daemon.rs:42`); blast-radius ignores an
  unreadable `--timings` file (`src/main.rs:257`); build-version falls back to
  the bare crate version when the stamp env vars are unset
  (`packages/build-version/src/lib.rs:46`).

- **Materialized image bytes are verified against the description.**
  `oci-image-builder materialize` regenerates each layer deterministically and
  checks it against the recorded digest, so a description that no longer
  reproduces its bytes fails rather than ships a wrong image
  (`packages/oci-image-builder/README.md:33`).

- **Generated Nix is reproducible and deterministic.** nix-cargo-unit sorts
  every hashed set (dependencies, features, crate types) before digesting
  (`src/model.rs:620`) and the template emits a fixed shape; the renderer is a
  pure stdin->stdout transform driven only by the unit graph, `Cargo.lock`, and
  flags.

## Glossary

- **unit / Cargo unit**: one node of `cargo build --unit-graph`: a single crate
  compiled in one mode (build/check/test/doc/run-custom-build) with one feature
  set and profile. The atom nix-cargo-unit turns into a derivation.
- **unit graph**: Cargo's `--unit-graph` JSON (version 1): `units`, `roots`, and
  per-unit `dependencies` by index. nix-cargo-unit parses, merges, and renders it.
- **identity hash**: the 16-hex SHA-256 prefix nix-cargo-unit derives per unit
  from its package/target/profile/features/flags plus its sorted dependency
  hashes; the stable tag in unit names and `-C metadata`.
- **content-addressed (CA) derivation**: a Nix derivation whose output path is
  assigned only after realisation; its `.drv` carries an empty output path and
  Nix builds it via a `resolved derivation` rewrite. The `--content-addressed`
  render flag, nom's patch, and the monitor's resolve-folding all exist for these.
- **internal-json**: Nix's `--log-format internal-json` event stream (`start`,
  `stop`, `result`, `msg` actions); what both monitors consume.
- **layer plan / `conf.json`**: the `passthru.conf` that
  `dockerTools.streamLayeredImage` emits, naming base image, store layers, and
  the customisation layer. oci-image-builder's input.
- **describe / materialize**: oci-image-builder's two passes: a tiny
  content-addressed `image.json` description (no layer bytes), and the
  regeneration of bytes from it back into an OCI tar.
- **blast radius**: the set of `.#checks.x86_64-linux` derivations a PR would
  rebuild, attributed to the changed-input frontier that caused them.
- **frontier (root cause)**: a derivation that differs between base and head but
  whose own inputs are all unchanged: the genuine cause, not a propagating
  intermediate (`packages/blast-radius/src/causes.rs`).
- **package.nix metadata**: the small attrset every `packages/**` unit declares
  (`id`, `flake`, `packageSet`, `overlay`, `inRustWorkspace`, `passthruTests`,
  ...) that `registry.nix` reads to build the flake.

## Components

| component | page | what |
| --- | --- | --- |
| nix-cargo-unit | [nix-cargo-unit/overview.md](nix-cargo-unit/overview.md) | Cargo unit graph -> per-unit Nix derivations; merge; panic scan |
| nix-web-monitor | [nix-web-monitor/overview.md](nix-web-monitor/overview.md) | run a Nix build with a live web monitor; internal-json parser + state model |
| nix-output-monitor | [nix-output-monitor/overview.md](nix-output-monitor/overview.md) | `nom` patched to parse content-addressed derivations |
| oci-image-builder | [oci-image-builder/overview.md](oci-image-builder/overview.md) | layer plan -> OCI tar / content-addressed image.json |
| blast-radius | [blast-radius/overview.md](blast-radius/overview.md) | PR rebuild-impact report over `.#checks` with root-cause attribution |
| registry.nix | [registry/overview.md](registry/overview.md) | package.nix discovery driving flake/overlay/packageSet lists |
| snix | [snix/overview.md](snix/overview.md) | snix `default` CLI built through cargo-unit |
| build-version | [build-version/overview.md](build-version/overview.md) | `--version` line from Nix-stamped build metadata |
| config-launch | [config-launch/overview.md](config-launch/overview.md) | spec-driven exec launcher: env/PATH/flag injection |
| progress-style | [progress-style/overview.md](progress-style/overview.md) | shared indicatif progress-bar/spinner styles |
