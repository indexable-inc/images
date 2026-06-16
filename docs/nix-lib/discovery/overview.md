# discovery and registration glue

The wiring that turns directory trees into flake outputs. Five files:
`lib/default.nix` (assembly), `lib/discovery.nix` (tree walks),
`lib/packages.nix` (flake-output package set), `lib/overlay.nix` (nixpkgs
overlay), and `lib/per-system.nix` (per-system outputs), plus the package index
at `packages/registry.nix`. Together they mean adding a package, module, image,
or example is a filesystem edit, not a registry edit.

## `packages/registry.nix`: the package index

Walks `packages/**` for every directory containing a `package.nix` metadata
file (`packages/registry.nix:24`, `116-141`) and returns the index every other
glue file resolves package ids through (`packages/registry.nix:179-190`):

- `byId.<id>` and `entries`: all package metadata records.
- `packageSetEntriesFor system` / `flakeEntriesFor system` /
  `overlayEntriesFor system`: entries enabled for a system, filtered by each
  entry's optional `systems` list (`packages/registry.nix:148-156`).
- `rustWorkspaceEntries`, `updateScriptEntriesFor`, `passthruTestEntriesFor`.

A `package.nix` is an attrset (or `{ lib }:` function) with `id` required and
the allowed keys `flake`, `inRustWorkspace`, `overlay`, `packageSet`,
`passthruTests`, `path`, `updateScript` (`packages/registry.nix:30-39`); an
unknown key throws (`assertKnownKeys`, `packages/registry.nix:41-49`), as do
duplicate ids (`packages/registry.nix:176-178`). `packageSet`/`flake`/`overlay`
are normalized into target descriptors: `true` takes the id as the selector, an
attrset overrides the selector key and/or `systems`
(`normalizeTarget`, `packages/registry.nix:55-100`).

## `lib/discovery.nix`: tree walks

`discoverTree` (`lib/discovery.nix:20-79`) is the shared walker. Given a `root`,
`requiredFiles` (default `[ "default.nix" ]`), an optional `metadataFile`
sidecar, and a `validate` hook, it returns `{ <name> = { path; metadata; }; }`
for every directory holding the required files. Rules: directories named with a
leading `_` are skipped with their subtree (`lib/discovery.nix:34`); each entry
`claims` its `name` plus any `outputNames` the validator adds; duplicate claims
across the tree throw with the offending paths (`lib/discovery.nix:67-76`).

Three consumers wrap it:

- `discoverImages { root, imageTests ? {} }` (`lib/discovery.nix:124-166`):
  walks `images/<category>/<name>/`. A `versions.nix` sidecar makes each version
  key a `<name>_<ver>` package plus a `<name>` alias for the `default` version
  (`imagePackages`, `lib/discovery.nix:85-111`). `imageTests.<name>` attaches an
  eval test as `passthru.tests.eval` (RFC 0119).
- `discoverModules { root }` (`lib/discovery.nix:184-218`): walks
  `modules/<category>/<name>/`, returns a nested attrset of module paths. A
  directory with descendant modules gets a `default` key so
  `services/minecraft/` ships `{ default = ...; fabric = ...; mods = {...}; }`.
  Built with `strictList` so overlapping subtrees throw.
- `exampleFleetsFor { hostSystem }` (`lib/discovery.nix:239-258`): walks both
  `examples/<name>/` and `examples/<category>/<name>/`, imports each with
  `{ index = { lib = ix // { mkFleet = mkFleetFor hostSystem; ... }; }; }`.

`lib/default.nix` calls `discoverModules` at eval (`lib/default.nix:78`) to build
`nixosModules`; `lib.collect builtins.isPath` flattens it to `moduleList`
(`lib/default.nix:94`), pulled into every image unconditionally so every option
is in scope and inert until enabled.

## `lib/packages.nix`: flake-output package set

`packageSetFor pkgs` (curried, `lib/packages.nix:11`) builds the
`packages.<system>.*` set. For each registry `packageSetEntriesFor` entry it
`callPackageWith`s the package's `path` against `pkgs // context`, where
`context` carries `ix` (the `ixSpecialArgs` bundle rebound to the caller's
`pkgs`, with `cargoUnit`/`goUnit`/`rustWorkspace` rebound too,
`lib/packages.nix:14-23`), `clippy-fork`, `ghostty`, and `repoPackages` (the
lazy fix-point of the set itself, so a package can depend on a sibling by id,
`lib/packages.nix:41-53`). Trees are assembled with `strictList`
(`lib/packages.nix:56-57`). These are reachable as
`packages.<system>.<name>` but never injected into the nixpkgs namespace.

## `lib/overlay.nix`: the repo nixpkgs overlay

`final: prev:` overlay (`lib/overlay.nix:15`) exposing the few repo-owned
packages that NixOS modules expect at `pkgs.<name>`. It reads the target system
from `prev` (not `final`, to avoid a recursion through `stdenv`,
`lib/overlay.nix:17-26`), then for each `overlayEntriesFor` registry entry builds
the package: via `entry.overlay.build context` if the metadata supplies a custom
builder, else `callPackageWith` (`lib/overlay.nix:39-48`). The `ix` arg threaded
in is the `sharedHelpers` surface so overlay packages resolve `ix.deepMerge` etc.
exactly as flake-output packages do (`lib/overlay.nix:7-13`). Also pins
`ixDefaultJre` from `lib/languages/jvm-defaults.nix` (`lib/overlay.nix:59-62`).
Exposed as `overlays.default` (`flake.nix:325`).

## `lib/per-system.nix`: per-system outputs

Evaluated once per dev system (`flake.nix:258-269`). Instantiates nixpkgs with
`rust-overlay.overlays.default` and `ix.overlay` (`lib/per-system.nix:18-24`),
binds `repoPackages = ix.packageSetFor pkgs` (`lib/per-system.nix:459`), and
defines the repo workflow tools as `nix run`-able packages
(`lib/per-system.nix:1-8`): `lint` (a DAG of nixfmt/statix/deadnix/astlog stages
via `dag-runner`, `lib/per-system.nix:35-124`), `check` (the full CI gate:
`nix-fast-build` over `ciChecks` then `nix-eval-jobs` over `packages`,
`lib/per-system.nix:165-293`), `update-mods`/`update-loaders`/`update-sounds`,
`mc-source`, the `bench`/`site`/`health-checks` apps, and the auto-discovered
`update` aggregator (registry `updateScript` flag). The file (~1150 lines) also
assembles `checks`, `ciChecks`, `apps`, `devShells`, and `formatter`.

## `lib/default.nix`: the assembly

One recursive `let` (`lib/default.nix:25-528`). It imports each helper from its
subdir, builds `packageRegistry` (`lib/default.nix:30-33`), the `overlay`/`pkgs`
(`lib/default.nix:55-74`), the language/rust/image/discovery surfaces, then
returns `ixReturn` (`lib/default.nix:491-528`). The three exported surfaces
(`sharedHelpers`, `ixSpecialArgs`, `ixReturn`) are described in
[common.md](../common.md). `ixReturn` is self-referential: `exampleFleetsFor`
passes it back into examples as `index.lib` (`lib/default.nix:488-490`).

## Adding things

- A package: `mkdir packages/<name>` + write `package.nix` (with `id`) and a
  `default.nix`/`package.nix` builder. Set `packageSet = true` for a flake
  output, `overlay = true` for a `pkgs.<name>`, `flake = true` for both,
  `inRustWorkspace = true` to join the shared Rust unit graph.
- A module: `mkdir modules/<category>/<name>` + `default.nix`. Picked up by
  `discoverModules`; `_`-prefixed siblings are ignored.
- An image: `mkdir images/<category>/<name>` + `default.nix` (+ optional
  `versions.nix`). Picked up by `discoverImages`.
- An example fleet: `mkdir examples/<category>/<name>` + `default.nix`.
