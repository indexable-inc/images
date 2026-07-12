{ix, ...}:
ix.cargoUnit.selectBinaryWithTests ix.rustWorkspace.units {
  binary = "efx";
  meta.mainProgram = "efx";
}
