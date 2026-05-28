{
  lib,
  packageRegistry,
  rust-overlay,
  symphony,
  buildIxRustTool,
  buildRustPackage,
  clippy-fork,
  writePythonApplication,
}:
final: _prev:
let
  packageSystem = final.stdenv.hostPlatform.system;
  rustPkgs = final.extend rust-overlay.overlays.default;
  symphonyRoomServerChecked = buildRustPackage rustPkgs {
    pname = "room-server";
    version = "0.1.0";
    src = symphony;
    cargoLock = {
      lockFile = symphony + "/Cargo.nix.lock";
    };
    cargoBuildFlags = [
      "-p"
      "room-server"
    ];
    cargoTestFlags = [
      "-p"
      "room-server"
    ];
    meta.mainProgram = "room-server";
  };
  symphonyRoomServerRaw = symphonyRoomServerChecked.passthru.unchecked;
  symphonyRoomServer =
    final.runCommand "room-server-wrapped"
      {
        nativeBuildInputs = [ final.makeWrapper ];
        meta = (symphonyRoomServerRaw.meta or { }) // {
          mainProgram = "room-server";
        };
      }
      ''
        mkdir -p $out/bin
        makeWrapper ${symphonyRoomServerRaw}/bin/room-server $out/bin/room-server \
          --prefix PATH : ${lib.makeBinPath [ final.codex ]} \
          --set-default ROOM_CODEX_BIN ${lib.getExe final.codex}
      '';
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
  symphony-room-server = symphonyRoomServer;
}
