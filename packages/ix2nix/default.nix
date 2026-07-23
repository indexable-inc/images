{
  ix,
  lib,
  ...
}:
ix.cargoUnit.selectBinaryWithTests ix.rustWorkspace.units {
  binary = "ix2nix";
  meta = {
    description = "Converts .ix modules (JavaScript syntax as a 1:1 skin over Nix semantics) into Nix source";
    license = lib.licenses.mit;
    mainProgram = "ix2nix";
  };
}
