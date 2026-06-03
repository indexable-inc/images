{ ix, ... }:

ix.cargoUnit.selectBinaryWithTests ix.rustWorkspace.units {
  binary = "otlp-mixedbread";
  meta.mainProgram = "otlp-mixedbread";
}
