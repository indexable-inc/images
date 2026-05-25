{
  ix,
  pkgs ? ix.pkgs,
}:

ix.buildUvApplication pkgs {
  pname = "python-mcp-server";
  version = "0.1.0";
  srcRoot = ./.;
  mainProgram = "ix-python-mcp";
}
