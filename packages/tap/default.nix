{ ix, ... }:

ix.cargoUnit.selectBinaryWithTests ix.rustWorkspace.units {
  binary = "tap";
  meta.mainProgram = "tap";
}
