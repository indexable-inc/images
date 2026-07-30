{
  ix,
  lib,
  ...
}:
ix.cargoUnit.selectBinaryWithTests ix.rustWorkspace.units {
  binary = "nix-dev-build";
  meta = {
    description = "Incremental meson/ninja builds of a nix source checkout";
    license = lib.licenses.mit;
    mainProgram = "nix-dev-build";
  };
}
