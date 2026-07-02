# The host binary out of the shared workspace unit graph (same shape as
# ../compositor/default.nix): package.nix carries the registry metadata, this
# file only selects the target.
{ ix, ... }:

ix.cargoUnit.selectBinaryWithTests ix.rustWorkspace.units {
  binary = "panes-host";
  meta = {
    description = "macOS agent presenting guest-Linux windows as native NSWindows";
    mainProgram = "panes-host";
    platforms = [ "aarch64-darwin" ];
  };
}
