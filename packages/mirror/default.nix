{
  ix,
  lib,
  ...
}:
ix.cargoUnit.selectBinaryWithTests ix.rustWorkspace.units {
  binary = "mirror";
  meta = {
    description = "Generate and sync standalone read-only GitHub mirror repos for opt-in workspace packages";
    license = lib.licenses.mit;
    mainProgram = "mirror";
  };
}
