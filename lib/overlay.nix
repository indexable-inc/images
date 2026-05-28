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
    in
    if entry.overlay ? build then
      entry.overlay.build context
    else
      final.callPackage entry.path (entry.overlay.callPackageArgs context);
in
lib.listToAttrs (
  map (entry: lib.nameValuePair entry.overlay.attrName (buildOverlayPackage entry)) (
    packageRegistry.overlayEntriesFor packageSystem
  )
)
// {
  symphony-room-server = symphony.packages.${packageSystem}.room-server;
}
