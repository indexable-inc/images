{ ix, lib, ... }:

ix.cargoUnit.selectBinaryWithTests ix.rustWorkspace.units {
  binary = "config-launch";
  meta = {
    description = "Config-driven exec launcher: inject CLI --config flags (forced always, soft only when absent from a config file) then exec a target, preserving argv0";
    license = lib.licenses.mit;
    mainProgram = "config-launch";
  };
}
