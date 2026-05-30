{ ix, ... }:

ix.cargoUnit.selectBinaryWithTests ix.rustWorkspace.units {
  binary = "semantic-search";
  meta.mainProgram = "semantic-search";
}
