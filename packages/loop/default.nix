{
  ix,
  lib,
  pkgs,
  viewer,
}:

let
  loop.binary = ix.cargoUnit.buildBinary {
    pname = "loop";
    src = ix.rustWorkspace.src;
    workspaceRoot = ix.rustWorkspace.root;
    cargoLock = ix.rustWorkspace.cargoLock;
    cargoArgs = [
      "-p"
      "loop"
    ];
    binary = "loop";
    policy = {
      denyUnusedCrateDependencies = false;
      cargoAudit.enable = false;
      cargoMachete.enable = false;
      clippy.enable = false;
    };
  };
in
pkgs.runCommand "loop"
  {
    nativeBuildInputs = [ pkgs.makeWrapper ];
    passthru.unwrapped = loop.binary;
    meta = {
      description = "Run agent loops and health checks with a Loro-backed web UI";
      mainProgram = "loop";
      license = lib.licenses.mit;
    };
  }
  ''
    mkdir -p "$out/bin"
    makeWrapper "${loop.binary}/bin/loop" "$out/bin/loop" \
      --set LOOP_VIEWER_DIR "${viewer}/share/loop-viewer"
  ''
