{
  ix,
  lib,
  ...
}:
ix.cargoUnit.selectBinaryWithTests ix.rustWorkspace.units {
  binary = "ix-wrap";
  meta = {
    description = "Transparent usage-counting exec wrapper driven by an IX_USAGE_SPEC JSON file";
    license = lib.licenses.mit;
    mainProgram = "ix-wrap";
  };
}
