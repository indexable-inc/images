{
  ix,
  lib,
  ...
}:
ix.cargoUnit.selectBinaryWithTests ix.rustWorkspace.units {
  binary = "darwin-deploy";
  meta = {
    description = "Deploy nix-darwin configurations to remote macOS hosts over ssh";
    license = lib.licenses.mit;
    mainProgram = "darwin-deploy";
  };
}
