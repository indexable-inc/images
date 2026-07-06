# Generic registry-driven package-set assembly, shared by index's own
# `lib/packages.nix` and by a downstream consumer (ix) that discovers its
# packages with the same `package.nix`-marker registry (`packages/registry.nix`,
# exposed as `ix.mkPackageRegistry`). The registry walk and the "callPackage each
# entry, place it at its `packageSet.attrPath`" assembly are index-owned
# machinery; the per-repo `context`/`ix` bundle threaded into each package is
# not, so the caller supplies it through `autoArgsFor`.
#
# This is the one implementation of the assembly loop. index's `lib/packages.nix`
# builds its index-specific `context` and calls straight through here; ix does
# the same with its own context, so neither repo re-forks the loop.
{lib}: let
  inherit (import ./util/deep-merge.nix {inherit lib;}) strictList;
in
  {
    # A `packages/registry.nix` result (from `mkPackageRegistry { root; }`).
    packageRegistry,
    # The package set the entries build against; also names the system the
    # registry filters `packageSet` targets by.
    pkgs,
    # `entry -> repoPackages -> attrs`: the caller-owned argument bundle handed
    # to `callPackageWith` for one entry. `repoPackages` is the assembled set
    # itself (a lazy fix-point), so an entry can depend on a sibling by id.
    # `entry` is the registry entry (id, path, packageSet, ...). The caller
    # decides what `pkgs`/`ix`/context to merge in; this loop only wires the
    # self-reference and placement.
    autoArgsFor,
  }: let
    packageSystem = pkgs.stdenv.hostPlatform.system;
    buildEntry = entry: lib.callPackageWith (autoArgsFor entry packageSet) entry.path {};
    packageTreeFor = entry: lib.setAttrByPath entry.packageSet.attrPath (buildEntry entry);
    packageSet = strictList (map packageTreeFor (packageRegistry.packageSetEntriesFor packageSystem));
  in
    packageSet
