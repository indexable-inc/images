{
  ix,
  lib,
  ...
}:
ix.cargoUnit.selectBinaryWithTests ix.rustWorkspace.units {
  binary = "sqlmerge";
  meta = {
    description = "git merge driver for SQLite databases: three-way merge via the session extension";
    license = lib.licenses.mit;
    mainProgram = "sqlmerge";
  };
}
