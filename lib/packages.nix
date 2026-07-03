{
  lib,
  packageRegistry,
  ixSpecialArgs,
  cargoUnitFor,
  goUnitFor,
  rustWorkspaceFor,
  clippy-fork,
  ghostty,
}: pkgs: let
  packageSystem = pkgs.stdenv.hostPlatform.system;
  ixForPackages =
    ixSpecialArgs
    // {
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
    # The Nushell writer pre-applied to the caller's pkgs, for packages that
    # build a checked Nushell command directly (e.g. chrome-vm, astlog scan).
    writeNushellApplication = ixForPackages.writeNushellApplication pkgs;
    # Same writer, exposed under a capability-oriented name for the nullable
    # updateScript path: a writer the package can use to build its
    # `passthru.updateScript`, pre-applied to the caller's pkgs so flake-output
    # packages need not re-thread `ix` through callPackage. The overlay path
    # leaves it unset, which is the signal to omit the updater.
    updateScriptWriter = ixForPackages.writeNushellApplication pkgs;
  };
  inherit (import ./util/deep-merge.nix {inherit lib;}) strictList;
  buildEntry = entry: let
    # `repoPackages` is the package set itself (a lazy fix-point), so an
    # entry can depend on a sibling by id (e.g. packages/agent/claude-code reads
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
    lib.callPackageWith autoArgs entry.path {};
  packageTreeFor = entry: lib.setAttrByPath entry.packageSet.attrPath (buildEntry entry);
  packageSet = strictList (map packageTreeFor (packageRegistry.packageSetEntriesFor packageSystem));
in
  packageSet
