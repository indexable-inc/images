{
  lib,
  packageRegistry,
  ixSpecialArgs,
  cargoUnitFor,
  goUnitFor,
  rustWorkspaceFor,
  clippy-src,
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
      # unibind build glue bound to the caller's pkgs, for the same reason as
      # rustWorkspace above.
      unibind = ixSpecialArgs.unibindFor pkgs;
      # Patched-source builder bound to the caller's pkgs, so codex / btop /
      # llm-clippy apply their patch series against a source that builds for the
      # consuming system rather than the top-level x86_64-linux one.
      patchedSrc = ixSpecialArgs.patchedSrcFor pkgs;
    };
  context = {
    inherit
      pkgs
      packageSystem
      clippy-src
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
  mkPackageSet = import ./mk-package-set.nix {inherit lib;};
in
  # index's own package set, assembled through the shared registry-driven loop
  # (`lib/mk-package-set.nix`). The only index-specific part is `autoArgsFor`:
  # `repoPackages` is threaded under one name (not merged flat) so ids that
  # shadow nixpkgs attrs (`btop`, `kitty`, ...) cannot hijack another package's
  # arguments and a same-named nixpkgs override does not resolve to itself.
  mkPackageSet {
    inherit packageRegistry pkgs;
    autoArgsFor = entry: repoPackages:
      pkgs
      // context
      // {
        inherit entry repoPackages;
      };
  }
