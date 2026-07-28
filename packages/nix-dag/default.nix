{
  ix,
  lib,
  ...
}:
ix.cargoUnit.selectBinaryWithTests ix.rustWorkspace.units {
  binary = "nix-dag";
  meta = {
    description = "Score a Nix build plan's shape: blast radius, fan-out, critical path, and env-carrier invalidation";
    license = lib.licenses.mit;
    mainProgram = "nix-dag";
  };
}
