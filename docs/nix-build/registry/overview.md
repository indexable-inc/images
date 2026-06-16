# registry.nix

`packages/registry.nix` is the package-registry helper: a single Nix file that
discovers every unit under `packages/**` by reading its `package.nix` metadata
and produces the per-system lists the flake uses to assemble its outputs. It is a
Nix-only library, not a flake output and not a package itself.

## Purpose

Rather than enumerate flake outputs by hand, the repo declares per-package intent
in a small `package.nix` next to each unit, and `registry.nix` walks the tree,
validates that metadata, and answers "which packages are flake outputs / overlay
entries / package-set entries / Rust workspace members / have passthru tests, on
this system". It is imported by `lib/default.nix` and `lib/per-system.nix` as
`packageRegistry` (`lib/default.nix:30`, `lib/per-system.nix:26`).

## Signature and discovery

Called as `import packages/registry.nix { lib; root; }` where `root` is the
packages directory (`registry.nix:1`). It recursively finds every directory
containing a `package.nix` (`dirsWithFile`/`packageDirs`, `:16-24`) and, for
diagnostics, every `default.nix` dir lacking a `package.nix`
(`packageDirsWithoutMetadata`, `:25-28`).

## package.nix metadata schema

Each `package.nix` is an attrset (or a `{ lib }`-function returning one) whose
keys are validated against `allowedMetadataKeys` (`:30-39`); an unknown key is a
hard assertion failure (`assertKnownKeys`, `:41-49`). Recognized keys:

- `id` (required): the unique package id; duplicate ids across the tree fail
  (`:176-178`).
- `flake`: expose a flake output. `true` takes the id as the attr name; an
  attrset overrides `attrName` and/or `systems` (`normalizeFlake`, `:89`).
- `overlay`: an overlay entry, with an extra `build` key (`normalizeOverlay`,
  `:95`).
- `packageSet`: a package-set entry selected by `attrPath` (defaults to `[ id ]`;
  `normalizePackageSet`, `:83`).
- `inRustWorkspace`: a member of the root Cargo workspace (built through
  [nix-cargo-unit](../nix-cargo-unit/overview.md) via `ix.cargoUnit`) (`:134`).
- `passthruTests`: expose passthru tests under a prefix (default `rust-<id>`;
  `normalizePassthruTests`, `:102`).
- `updateScript`: marks a package exposing a `passthru.updateScript`, driving the
  generated `update` aggregator (`:136-140`).
- `path`: override the source path (defaults to the discovered dir, `:128`).

`flake`/`overlay`/`packageSet`/`passthruTests` are all system-scoped: a target is
`null`/`false` to disable, `true` for the default selector, or an attrset to
override the selector key and `systems` (`normalizeTarget`, `:55-81`).

## Outputs

The returned attrset (`registry.nix:179-190`):

- `entries`: every package's normalized metadata; `byId`: the same keyed by id.
- `packageDirsWithoutMetadata`: `default.nix` dirs missing a `package.nix`.
- `packageSetEntriesFor system`, `flakeEntriesFor system`,
  `overlayEntriesFor system`, `updateScriptEntriesFor system`,
  `passthruTestEntriesFor system`: the per-system filtered lists
  (`enabledForSystem`, `:148-150`, gates on each target's `systems`).
- `rustWorkspaceEntries`: every `inRustWorkspace` package.

`passthruTestEntriesFor` includes a package when its `packageSet` is enabled for
the system, or (for a pure workspace crate) when it is `inRustWorkspace`
(`:164-172`), so library crates like [build-version](../build-version/overview.md)
and [progress-style](../progress-style/overview.md) still get gated.

## Relation to the rest of the domain

Every other unit in this domain declares its `package.nix` here: nix-cargo-unit
(`packageSet`/`flake`, not `inRustWorkspace`), the Rust workspace tools
(`inRustWorkspace` + `flake`), snix and nix-output-monitor (`flake`/`packageSet`,
not in the workspace), and the library crates (`inRustWorkspace` only). This file
is what turns those declarations into the flake's `packages`, `overlays`, and
check lists.
