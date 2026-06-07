{
  lib,
  packageRegistry,
  buildIxRustTool,
  clippy-fork,
  writePythonApplication,
  # Curated cross-cutting helper surface (`lib`'s `sharedHelpers`), threaded in
  # as the `ix` arg so overlay-built packages can reach pure helpers like
  # `ix.deepMerge` exactly as flake-output packages do (lib/packages.nix binds
  # `ix` for the `packageSetFor` path). Without this, an overlay package that
  # takes an `ix` argument fails callPackage with a missing-arg error (e.g.
  # packages/claude-code uses `ix.deepMerge.rhs`).
  ix,
}:
final: _prev:
let
  packageSystem = final.stdenv.hostPlatform.system;
  overlayContext = entry: {
    inherit
      entry
      final
      buildIxRustTool
      clippy-fork
      ix
      ;
    pkgs = final;
    inherit (entry) path;
    writePythonApplication = writePythonApplication final;
  };
  buildOverlayPackage =
    entry:
    let
      context = overlayContext entry;
      autoArgs = final // context;
    in
    if entry.overlay ? build then
      entry.overlay.build context
    else
      lib.callPackageWith autoArgs entry.path { };
in
lib.genAttrs' (packageRegistry.overlayEntriesFor packageSystem) (
  entry: lib.nameValuePair entry.overlay.attrName (buildOverlayPackage entry)
)
// {
  # TODO: re-add symphony-room-server. The room-server binary lives in the ix
  # monorepo (`ix#packages.x86_64-linux.room-server`); the ix<->index flake
  # cycle blocks sourcing it from there, so the old `symphony` input pin was
  # removed. images/dev/symphony-codex consumes this when re-added.

  # Default Temurin JRE for repo-owned package sets. The major lives in
  # `lib/languages/jvm-defaults.nix`, shared with `ix.languages.{java,scala}`
  # and exported NixOS modules.
  ixDefaultJre = final."temurin-jre-bin-${import ./languages/jvm-defaults.nix}";
}
