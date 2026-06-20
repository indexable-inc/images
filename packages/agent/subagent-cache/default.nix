{ ix, lib, ... }:

ix.cargoUnit.selectBinaryWithTests ix.rustWorkspace.units {
  binary = "subagent-cache";
  meta = {
    description = "Content-validated cache daemon for read-only subagent investigations (FTS recall + Haiku judge over Postgres)";
    license = lib.licenses.mit;
    mainProgram = "subagent-cache";
  };
}
