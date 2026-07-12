{
  ix,
  lib,
  ...
}:
ix.cargoUnit.selectBinaryWithTests ix.rustWorkspace.units {
  binary = "git-log-pretty";
  meta = {
    description = "Pretty git log viewer that lists commits ahead of main with file-icon trees";
    license = lib.licenses.mit;
    mainProgram = "git-log-pretty";
  };
}
