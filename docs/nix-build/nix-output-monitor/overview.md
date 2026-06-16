# nix-output-monitor

`packages/nix-output-monitor` is the upstream `nix-output-monitor` (`nom`),
re-packaged with one patch so it parses content-addressed derivations. It is a
Nix-only package: no Rust, no source crate, just a Haskell-package override and a
smoke test.

## Purpose

`nom` wraps a Nix build and renders a live terminal dependency graph: which
derivations are building, queued, downloaded, and done. The `index` repo builds
heavily with content-addressed derivations (the `--content-addressed` render flag
of [nix-cargo-unit](../nix-cargo-unit/overview.md)), and stock `nom` cannot draw
a graph for them: it crashes on every CA `.drv`. This package fixes that so `nom`
is usable on this repo's builds. For the in-house web alternative see
[nix-web-monitor](../nix-web-monitor/overview.md).

## The bug and the patch

`nom build` reads every `.drv` with the `nix-derivation` Haskell library. Its
1.1.3 parser runs each output path through `filepathParser`, which fails on an
empty string. A floating content-addressed (or deferred) output is exactly
`("out","","r:sha256","")` with an empty path (the store path is assigned only
after realisation), so `nom` spams `DerivationParseError "string"` and renders no
dependency graph for CA derivations (`default.nix:7-13`).

`ca-empty-output-path.patch` widens `filepathParser` to accept an empty path
(returning the empty string) alongside the existing valid-absolute-path case
(`ca-empty-output-path.patch:9-15`). `nom` is then unchanged: its
`insertDerivation` calls `parseStorePath ""`, which returns `Nothing`, so the
still-unrealised floating output is dropped via `traverseMaybeWithKey` instead of
crashing on a partial field selector (`default.nix:14-21`). This is deliberately
smaller than upstream PR #26, which turns `DerivationOutput` into a sum type and
would force a matching `nom` source patch.

## How it builds (`default.nix`)

- Extends `pkgs.haskellPackages` so `nix-derivation` carries the patch via
  `haskell.lib.compose.appendPatch` (`:30-34`).
- nixpkgs builds `nom` as a top-level `callPackage` (not inside the
  `haskellPackages` set), so the override is fed through the top-level package's
  `haskellPackages` argument with `pkgs.nix-output-monitor.override`
  (`:23-36`), preserving by-name postInstall symlinks and completions.
- `overrideAttrs` attaches a `smoke` passthru test that runs `nom --help` and
  asserts the usage banner (`--version` shells out to `nix`, absent in the
  sandbox, so `--help` is used) (`:38-65`).
- `meta.mainProgram = "nom"`, description set (`:66-69`).

## Packaging

`package.nix`: `packageSet = true`, `flake = true`, `passthruTests = true`. Not a
Rust workspace member. Flake output: `nix-output-monitor`; binary: `nom`. Run as
`nix run .#nix-output-monitor -- build .#ix` (or `nix build ... |& nom`).

Upstream tracking: nix-output-monitor#122 and the Haskell-Nix-Derivation-Library
issue #28 referenced in `default.nix:28-29`.
