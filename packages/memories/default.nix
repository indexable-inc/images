{
  ix,
  lib,
  ...
}:
ix.cargoUnit.selectBinaryWithTests ix.rustWorkspace.units {
  binary = "memories";
  meta = {
    description = "Search, lint and write the per-repo `.memories` markdown corpus";
    license = lib.licenses.mit;
    mainProgram = "memories";
  };
}
