{
  ix,
  lib,
  ...
}:
ix.cargoUnit.selectBinaryWithTests ix.rustWorkspace.units {
  binary = "link-meta-demo";
  meta = {
    description = "Demo binary that declares a JSON stdout lens via embedded linking metadata";
    license = lib.licenses.mit;
    mainProgram = "link-meta-demo";
  };
}
