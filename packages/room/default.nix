{
  ix,
  lib,
  pkgs,
  site,
}:

let
  binary = ix.cargoUnit.buildBinary {
    pname = "room";
    src = ix.rustWorkspace.src;
    workspaceRoot = ix.rustWorkspace.root;
    cargoLock = ix.rustWorkspace.cargoLock;
    cargoArgs = [
      "-p"
      "room"
    ];
    binary = "room";
    policy = {
      denyUnusedCrateDependencies = false;
      cargoAudit.enable = false;
      cargoMachete.enable = false;
      clippy.enable = false;
    };
  };
in
pkgs.runCommand "room"
  {
    nativeBuildInputs = [ pkgs.makeWrapper ];
    passthru.unwrapped = binary;
    meta = {
      description = "Serve a multiplayer team room with shared presence and agent state";
      mainProgram = "room";
      license = lib.licenses.mit;
    };
  }
  ''
    mkdir -p "$out/bin"
    makeWrapper "${binary}/bin/room" "$out/bin/room" \
      --set ROOM_SITE_DIR "${site}/share/room-site"
  ''
