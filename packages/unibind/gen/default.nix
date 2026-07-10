{
  ix,
  lib,
  ...
}:
ix.cargoUnit.selectBinaryWithTests ix.rustWorkspace.units {
  binary = "unibind-gen";
  meta = {
    description = "Render host-language files (.pyi stubs, py.typed, wrapper modules) from the unibind IR embedded in a compiled artifact";
    license = lib.licenses.mit;
    mainProgram = "unibind-gen";
  };
}
