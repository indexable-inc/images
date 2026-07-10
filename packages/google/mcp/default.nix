{ix, ...}:
ix.cargoUnit.selectBinaryWithTests ix.rustWorkspace.units {
  binary = "ix-google-mcp";
  packageName = "google-mcp";
  meta.mainProgram = "ix-google-mcp";
}
