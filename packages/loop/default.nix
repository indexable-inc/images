{
  ix,
  lib,
  pkgs,
  viewer,
}:

let
  unwrapped = ix.cargoUnit.selectBinaryWithTests ix.rustWorkspace.units {
    binary = "loop";
  };
in
pkgs.runCommand "loop"
  {
    nativeBuildInputs = [ pkgs.makeWrapper ];
    passthru = unwrapped.passthru // {
      inherit unwrapped;
    };
    meta = {
      description = "Run agent loops and health checks with a Loro-backed web UI";
      mainProgram = "loop";
      license = lib.licenses.mit;
    };
  }
  ''
    mkdir -p "$out/bin"
    makeWrapper "${unwrapped}/bin/loop" "$out/bin/loop" \
      --set LOOP_VIEWER_DIR "${viewer}/share/loop-viewer"
  ''
