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
lib.listToAttrs (
  map (entry: lib.nameValuePair entry.overlay.attrName (buildOverlayPackage entry)) (
    packageRegistry.overlayEntriesFor packageSystem
  )
)
// {
  symphony-room-server = symphony.packages."${packageSystem}".room-server;
}
