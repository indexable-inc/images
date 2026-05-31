{ ix, ... }:

ix.cargoUnit.selectBinaryWithTests ix.rustWorkspace.units {
  binary = "claude-history-sync";
  meta.mainProgram = "claude-history-sync";
}
