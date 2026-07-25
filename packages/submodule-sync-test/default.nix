{
  ix,
  lib,
  ...
}:
ix.cargoUnit.selectBinaryWithTests ix.rustWorkspace.units {
  binary = "submodule-sync-test";
  meta = {
    description = "Integration test for update-flake-lock.yml's direct submodule sync: gitlink and lock advance together, a current pin is a no-op, and a lost push race retries onto the new tip";
    license = lib.licenses.mit;
    mainProgram = "submodule-sync-test";
  };
}
