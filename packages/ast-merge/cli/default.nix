{ ix, lib, ... }:

ix.cargoUnit.selectBinaryWithTests ix.rustWorkspace.units {
  binary = "ast-merge";
  meta = {
    description = "AST-aware git merge driver using tree-sitter";
    license = lib.licenses.mit;
    mainProgram = "ast-merge";
  };
}
