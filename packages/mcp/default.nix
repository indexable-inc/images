{
  ix,
  lib,
  pkgs ? ix.pkgs,
}:

let
  unwrapped = ix.cargoUnit.selectBinaryWithTests ix.rustWorkspace.units {
    binary = "ix-mcp";
  };
in
pkgs.runCommand "ix-mcp"
  {
    nativeBuildInputs = [ pkgs.makeWrapper ];
    passthru = unwrapped.passthru // {
      inherit unwrapped;
    };
    meta = (unwrapped.meta or { }) // {
      mainProgram = "ix-mcp";
    };
  }
  ''
    mkdir -p $out/bin
    makeWrapper ${unwrapped}/bin/ix-mcp $out/bin/ix-mcp \
      --set IX_MCP_DEFAULT_PYTHON ${lib.escapeShellArg (lib.getExe pkgs.python3)}
  ''
