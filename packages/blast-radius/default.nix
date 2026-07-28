{
  ix,
  lib,
  ...
}:
ix.cargoUnit.selectBinaryWithTests ix.rustWorkspace.units {
  binary = "blast-radius";
  meta = {
    description = "Report how many .#checks.x86_64-linux derivations a PR would rebuild, and which changed inputs caused each rebuild";
    license = lib.licenses.mit;
    mainProgram = "blast-radius";
  };
}
