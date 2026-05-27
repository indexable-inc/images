{ ix, ... }:

ix.cargoUnit.selectBinaryWithTests ix.rustWorkspace.units {
  binary = "file-search";
  meta.mainProgram = "file-search";
}
