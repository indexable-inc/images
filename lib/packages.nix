{
  lib,
  packageRegistry,
  ixSpecialArgs,
  cargoUnitFor,
  goUnitFor,
  rustWorkspaceFor,
  clippy-fork,
  ghostty,
}:
pkgs:
let
  packageSystem = pkgs.stdenv.hostPlatform.system;
  ixForPackages = ixSpecialArgs // {
    inherit pkgs;
    # Rebind the language unit builders to the caller's pkgs so repo
    # packages built through packageSetFor compile for the host system
    # instead of the x86_64-linux pkgs the top-level ixSpecialArgs bundle
    # is bound to.
    cargoUnit = cargoUnitFor pkgs;
    goUnit = goUnitFor pkgs;
    rustWorkspace = rustWorkspaceFor pkgs;
  };
  context = {
    inherit
      pkgs
      packageSystem
      clippy-fork
      ghostty
      ixForPackages
      ;
    ix = ixForPackages;
    # Pre-applied to the caller's pkgs so flake-output packages can build a
    # `passthru.updateScript` without re-threading `ix` through callPackage.
    writeNushellApplication = ixForPackages.writeNushellApplication pkgs;
  };
  inherit (import ./util/deep-merge.nix { inherit lib; }) strictList;
  buildEntry =
    entry:
    let
      # `repoPackages` is the package set itself (a lazy fix-point), so an
      # entry can depend on a sibling by id (e.g. packages/claude-code reads
      # `repoPackages.mcp`). Threaded under one name rather than merged flat
      # into autoArgs: a flat merge would let ids that shadow nixpkgs attrs
      # (`btop`, `kitty`, ...) hijack other packages' arguments, and a
      # same-named nixpkgs override would resolve to itself.
      autoArgs =
        pkgs
        // context
        // {
          inherit entry;
          repoPackages = packageSet;
        };
    in
    lib.callPackageWith autoArgs entry.path { };
  packageTreeFor = entry: lib.setAttrByPath entry.packageSet.attrPath (buildEntry entry);
  packageSet = strictList (map packageTreeFor (packageRegistry.packageSetEntriesFor packageSystem));
in
packageSet
