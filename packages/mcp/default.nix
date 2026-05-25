{
  ix,
  lib,
  pkgs ? ix.pkgs,
}:

ix.buildRustPackage pkgs {
  pname = "mcp";
  version = "0.1.0";

  src = ix.rustWorkspace.src;
  cargoLock.lockFile = ix.rustWorkspace.cargoLock;
  buildAndTestSubdir = "packages/mcp";
  cargoArgs = [
    "-p"
    "ix-mcp"
  ];

  nativeBuildInputs = [ pkgs.makeWrapper ];

  postInstall = ''
    wrapProgram "$out/bin/ix-mcp" \
      --set IX_MCP_DEFAULT_PYTHON ${lib.escapeShellArg (lib.getExe pkgs.python3)}
  '';

  meta.mainProgram = "ix-mcp";
}
