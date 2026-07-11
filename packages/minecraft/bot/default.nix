{
  ix,
  lib,
  ...
}:
ix.cargoUnit.selectBinaryWithTests ix.rustWorkspace.units {
  binary = "mc-bot";
  meta = {
    description = "Headless Minecraft client that records a session as a ReplayMod .mcpr replay";
    license = lib.licenses.mit;
    mainProgram = "mc-bot";
  };
}
