{
  ix,
  lib,
  ...
}:
ix.cargoUnit.selectBinaryWithTests ix.rustWorkspace.units {
  binary = "complexity";
  packageName = "complexity-cli";
  meta = {
    description = "Per-function complexity ranking and the repo complexity budget";
    license = lib.licenses.mit;
    mainProgram = "complexity";
  };
}
