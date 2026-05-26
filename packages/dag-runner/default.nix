{ ix, ... }:

ix.cargoUnit.selectBinaryWithTests ix.rustWorkspace.units {
  binary = "dag-runner";
  meta.mainProgram = "dag-runner";
}
