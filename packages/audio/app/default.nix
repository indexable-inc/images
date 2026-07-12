{
  ix,
  lib,
  ...
}:
ix.cargoUnit.selectBinaryWithTests ix.rustWorkspace.units {
  binary = "shared-audio";
  meta = {
    description = "P2P deterministic LAN audio: Loro score, shared clock, WASM instruments";
    license = lib.licenses.mit;
    mainProgram = "shared-audio";
  };
}
