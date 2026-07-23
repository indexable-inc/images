{ix, ...}:
ix.cargoUnit.selectBinaryWithTests ix.rustWorkspace.units {
  binary = "indexer";
  meta.mainProgram = "indexer";
}
