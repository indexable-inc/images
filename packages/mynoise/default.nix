{ ix, lib, ... }:

ix.cargoUnit.selectBinaryWithTests ix.rustWorkspace.units {
  binary = "mynoise";
  meta = {
    description = "Play myNoise.net generators by streaming and mixing their band loops locally";
    license = lib.licenses.mit;
    mainProgram = "mynoise";
  };
}
