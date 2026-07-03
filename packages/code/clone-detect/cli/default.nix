{
  ix,
  lib,
  ...
}:
ix.cargoUnit.selectBinaryWithTests ix.rustWorkspace.units {
  binary = "clone";
  packageName = "clone-cli";
  meta = {
    description = "Code clone and duplication detector (Type-1/2/3) over tree-sitter ASTs";
    license = lib.licenses.mit;
    mainProgram = "clone";
  };
}
