{
  ix,
  lib,
  ...
}:
ix.cargoUnit.selectBinaryWithTests ix.rustWorkspace.units {
  binary = "tree-sync";
  meta = {
    description = "Sync a source tree to a remote host or another checkout using git's view of the tree";
    license = lib.licenses.mit;
    mainProgram = "tree-sync";
  };
}
