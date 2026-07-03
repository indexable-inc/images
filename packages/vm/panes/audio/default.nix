# The audio daemon binary out of the shared workspace unit graph (dag-runner
# pattern): package.nix carries the registry metadata, this file only selects
# the target. Same selection pattern as ../compositor.
{ ix, ... }:

ix.cargoUnit.selectBinaryWithTests ix.rustWorkspace.units {
  binary = "panes-audio";
  meta = {
    description = "Guest daemon shipping the PipeWire PCM mix to the macOS host over vsock";
    mainProgram = "panes-audio";
    # The guest is aarch64; x86_64 exists so CI's x86_64-linux-only graph
    # compiles and tests the crate (see package.nix).
    platforms = [
      "aarch64-linux"
      "x86_64-linux"
    ];
  };
}
