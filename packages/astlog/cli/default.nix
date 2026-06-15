{ ix, lib, ... }:

ix.cargoUnit.selectBinaryWithTests ix.rustWorkspace.units {
  binary = "astlog";
  meta = {
    description = "Datalog over tree-sitter ASTs: query relations, join them, apply rewrites";
    license = lib.licenses.mit;
    mainProgram = "astlog";
  };
}
