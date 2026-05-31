{ ix, lib, ... }:

ix.cargoUnit.selectBinaryWithTests ix.rustWorkspace.units {
  binary = "macos-vm";
  meta = {
    description = "Drive Apple's Virtualization.framework from Rust: own a VM's lifecycle";
    license = lib.licenses.mit;
    mainProgram = "macos-vm";
    platforms = [ "aarch64-darwin" ];
  };
}
