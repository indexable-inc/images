{
  ix,
  lib,
  ...
}:
ix.cargoUnit.selectBinaryWithTests ix.rustWorkspace.units {
  binary = "ixterm";
  meta = {
    description = "ix-term session CLI: `ixterm open <path>` sends a private OSC 5522 open request to the session pts";
    license = lib.licenses.mit;
    mainProgram = "ixterm";
  };
}
