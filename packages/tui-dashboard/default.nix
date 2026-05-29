{ ix, ... }:

ix.cargoUnit.selectBinaryWithTests ix.rustWorkspace.units {
  binary = "tui-dashboard";
  meta.mainProgram = "tui-dashboard";
}
