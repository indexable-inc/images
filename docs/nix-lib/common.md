# nix-lib

`lib/` is the repo's Nix helper/builder/library layer: the functions, builders,
and attrsets that build the Rust workspace, per-language toolchains, OCI images,
fleets, systemd/home-manager service helpers, and the cross-cutting utilities
every package and module reaches for. `lib/default.nix` wires all of it into one
attrset (`ixReturn`, `lib/default.nix:491-528`), which the flake exposes as the
`lib` output (`flake.nix:273`) and threads into every module as
`specialArgs.ix`. If you write Nix in this repo, this is the surface you call.

Read this page first, then the component page for the area you are touching.

## How `lib` is assembled and exposed

`flake.nix` calls `import ./lib { ... }` once (`flake.nix:234-251`), passing
nixpkgs, the flake inputs (rust-overlay, home-manager, source-tree pins), the
flake `rev`/`revEpoch`, and a `paths` attrset (`flake.nix:206-232`) that is the
single source of truth for every path literal. `lib/default.nix` is one big
recursive `let`: each helper is imported from its subdir and the file's job is
to wire them together (`lib/default.nix:1-2`).

Three derived surfaces come out of that `let`, all built from one
`sharedHelpers` base (`lib/default.nix:388-433`) so a new helper reaches every
consumer from a single edit:

| surface | binding | who reads it |
| --- | --- | --- |
| `ixReturn` | `lib/default.nix:491` | the flake `lib` output (`flake.nix:273`); examples receive it as `index.lib` |
| `ixSpecialArgs` | `lib/default.nix:440` | every NixOS module as `specialArgs.ix` (`lib/image/default.nix:65`) |
| `sharedHelpers` | `lib/default.nix:388` | the common base both of the above splice with `//` |

The flake then maps `ixReturn` into outputs: `lib`, `nixosModules`,
`overlays.default`, `homeModules`, and (via `lib/per-system.nix` evaluated per
system in `flake.nix:258-269`) `packages`, `checks`, `ciChecks`, `apps`,
`devShells`, `formatter` (`flake.nix:272-341`).

`lib/per-system.nix` is the per-system half: it instantiates nixpkgs with the
rust-overlay and `ix.overlay`, then defines the workflow tools (`lint`,
`check`, `update`, `bench`, `health-checks`, `site`, ...) as
`packages.<system>.<name>` derivations with `meta.mainProgram` so each is both
`nix run`-able and `nix build`-able (`lib/per-system.nix:1-8`).

## Areas (`lib/` subdirs)

| area | what | page |
| --- | --- | --- |
| discovery glue | `default.nix` + `discovery.nix` + `packages.nix` + `per-system.nix` + `overlay.nix`: how packages/modules/images/examples are auto-discovered and wired | [discovery](discovery/overview.md) |
| `lib/rust` | cargo-unit: the core Rust builder (vendor, unit graph, per-unit build, policy gates, prebuilt injection, cross) | [rust](rust/overview.md) |
| `lib/languages` | per-language toolchain/compiler/interpreter selectors (rust, python, go, java, ...) | [languages](languages/overview.md) |
| `lib/image` | `mkImage`/`mkNonNixImage`/`mkFleet`/`mkDev`: NixOS->OCI builders, fleet eval, dev fleets, the base platform module | [image](image/overview.md) |
| `lib/services` | portable-services (launchd+systemd), systemd-hardening, mutable-json: home-manager and systemd helpers | [services](services/overview.md) |
| `lib/dev` | `ix.dev.*` options, agent CLI layer, identity binds, SMB shared mount for dev fleets | [dev](dev/overview.md) |
| `lib/darwin` | pinned macOS SDK + zig cross toolchain for Linux->Darwin Rust builds | [darwin](darwin/overview.md) |
| `lib/minecraft` | NBT tag constructors + format generator, dimension-type snapshots, loader module factory, sync-managed wrapper | [minecraft](minecraft/overview.md) |
| `lib/agent-context` | always-on instruction doc + progressive skills, skills/agents directory assembly, frontmatter parser | [agent-context](agent-context/overview.md) |
| `lib/util` | pure utilities: errors, deep-merge, endpoint, attrs, lists, toml, mcp, relative-path, secrets, writers, bench, artifacts | [util](util/overview.md) |
| `lib/build` | non-Rust language builders: bun/npm/uv lock vendoring, JS/Svelte sites, Go units, Gradle fat-jars, Zig, libghostty-vt | [build-helpers](build-helpers/overview.md) |

## Cross-component invariants

- **One `pkgs` per surface.** `lib/default.nix` pins `system = "x86_64-linux"`
  (`lib/default.nix:28`) and builds one overlaid `pkgs` for image/module eval
  (`lib/default.nix:74`). Image builds share one nixpkgs instance via
  `nixpkgs.pkgs` (`lib/image/default.nix:39-47`) rather than re-instantiating
  per node. Most builders are curried `pkgs: args:` so a caller can rebind them
  to the host system (`lib/packages.nix:14-23`).
- **The registry is the package index.** `packages/registry.nix` walks
  `packages/**` for `package.nix` metadata files and returns `byId`,
  `packageSetEntriesFor`, `flakeEntriesFor`, `overlayEntriesFor`, etc.
  (`packages/registry.nix:179-190`). `packageSetFor`, `overlay`, and
  `buildIxRustTool` all resolve packages by id through it. See
  [discovery](discovery/overview.md).
- **Discovery, not manifests.** `images/`, `modules/`, and `examples/` are
  walked by `discoverTree` (`lib/discovery.nix:20-79`): a directory with the
  required files becomes an output, duplicate output names throw, and `_`-
  prefixed dirs are skipped. Adding an image/module/example is a `mkdir` + edit,
  no registry change (`lib/discovery.nix:235-237`).
- **Validate-then-return helpers.** Language and many util helpers route bad
  args through `ix.errors` (`assertEnum`/`requireArg`/`requireAttr`,
  `lib/util/errors.nix`) so a typo throws with the valid set listed instead of
  `attribute missing` deep in eval.
- **`deepMerge` is the only sanctioned recursive merge.** `lib/util/deep-merge.nix`
  replaces hand-rolled merges and the `no-recursive-update` lint points at it;
  `strict` throws on leaf collision, `rhs` lets the override win,
  `strictList` folds N attrsets.
- **Content-addressed Rust units.** The Rust workspace units default to
  `contentAddressed = true` (`lib/rust/cargo-unit.nix:301`), which is why the
  flake declares `ca-derivations` in `nixConfig.extra-experimental-features`
  (`flake.nix:18-20`).

## Glossary

- **`specialArgs.ix`**: the `ixSpecialArgs` bundle handed to every NixOS module;
  the cross-module contract (helpers, `packages`, `buildRustPackage`,
  `islandsTheme`). Keep it small and stable (`lib/default.nix:435-443`).
- **`ix` / `index.lib`**: the public `ixReturn` attrset; `flake.nix` exports it
  as `lib`, examples consume it as `index.lib`.
- **cargo unit**: one Cargo rustc compile step (a crate target at a profile with
  fixed features/deps), built as its own Nix derivation. See [rust](rust/overview.md).
- **unit graph**: Cargo's `--unit-graph` JSON, the planning input the
  nix-cargo-unit renderer turns into one derivation per unit.
- **vendor dir**: a package-shaped directory (one entry per `Cargo.lock`
  dependency) plus the cargo config pointing at it, so builds fetch nothing
  (`lib/rust/vendor.nix`).
- **toolchain id**: the basename of a Rust toolchain store path, baked into
  every unit hash (`lib/rust/resolve.nix:37-41`).
- **policy**: the Rust quality/correctness gates (clippy, cargo-audit,
  cargo-machete, unused-dep denial, panic-freedom, tests, linker) resolved from
  a typed schema (`lib/rust/policy.nix`).
- **registry**: `packages/registry.nix`, the metadata index of every package
  under `packages/**` keyed by `id`.
- **sidecar**: an optional per-directory metadata file discovery imports
  (`versions.nix` for images, `package.nix` for packages).
- **overlay package vs flake-output package**: overlay packages enter the
  nixpkgs namespace as `pkgs.<name>` for modules (`lib/overlay.nix`);
  flake-output-only packages reach consumers as `packages.<system>.<name>`
  without leaking into images (`lib/packages.nix`).
- **fleet**: a Colmena-style set of VM nodes evaluated by `mkFleet`
  (`lib/image/fleet.nix`); each node is a NixOS image plus deployment metadata.
- **portable service**: one spec rendered to a native launchd agent (macOS) and
  systemd user units (Linux) (`lib/services/portable-services.nix`).
