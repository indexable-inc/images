{ ix, ... }:

ix.cargoUnit.selectBinaryWithTests ix.rustWorkspace.units {
  binary = "gcal";
  meta.mainProgram = "gcal";
}
