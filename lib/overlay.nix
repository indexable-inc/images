{
  lib,
  packageRegistry,
  symphony,
  buildIxRustTool,
  clippy-fork,
  writePythonApplication,
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
  symphony-room-server = symphony.packages."${packageSystem}".room-server;

  # Default Temurin JRE for repo-owned package sets. The major lives in
  # `lib/languages/jvm-defaults.nix`, shared with `ix.languages.{java,scala}`
  # and exported NixOS modules.
  ixDefaultJre = final."temurin-jre-bin-${import ./languages/jvm-defaults.nix}";
}
