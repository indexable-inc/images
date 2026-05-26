{
  ix,
  lib,
  pkgs,
  site,
}:

let
  unwrapped = ix.cargoUnit.selectBinaryWithTests ix.rustWorkspace.units {
    binary = "room";
  };
in
pkgs.runCommand "room"
  {
    nativeBuildInputs = [ pkgs.makeWrapper ];
    passthru = unwrapped.passthru // {
      inherit unwrapped;
    };
    meta = {
      description = "Serve a multiplayer team room with shared presence and agent state";
      mainProgram = "room";
      license = lib.licenses.mit;
    };
  }
  ''
    mkdir -p "$out/bin"
    makeWrapper "${unwrapped}/bin/room" "$out/bin/room" \
      --set ROOM_SITE_DIR "${site}/share/room-site"
  ''
