{ ix, ... }:

ix.cargoUnit.selectBinaryWithTests ix.rustWorkspace.units {
  binary = "indexbench";
  meta.mainProgram = "indexbench";
}
