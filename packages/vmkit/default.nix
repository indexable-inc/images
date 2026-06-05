{ ix, lib, ... }:

ix.cargoUnit.selectBinaryWithTests ix.rustWorkspace.units {
  binary = "vmkit";
  meta = {
    description = "Own a guest VM's lifecycle: macOS guests on Virtualization.framework, Linux guests on libkrun";
    license = lib.licenses.mit;
    mainProgram = "vmkit";
    platforms = [
      "aarch64-darwin"
      "aarch64-linux"
      "x86_64-linux"
    ];
  };
}
