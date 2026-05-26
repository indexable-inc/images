{ ix, ... }:

ix.cargoUnit.selectBinaryWithTests ix.rustWorkspace.units {
  binary = "ix-dev-diagnose";
  meta.mainProgram = "ix-dev-diagnose";
}
