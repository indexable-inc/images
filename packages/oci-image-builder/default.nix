{ix, ...}:
ix.cargoUnit.selectBinaryWithTests ix.rustWorkspace.units {
  binary = "oci-image-builder";
  meta.mainProgram = "oci-image-builder";
}
