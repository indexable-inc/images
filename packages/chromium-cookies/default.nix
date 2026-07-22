{
  ix,
  lib,
  ...
}:
ix.cargoUnit.selectBinaryWithTests ix.rustWorkspace.units {
  binary = "chromium-cookies";
  meta = {
    description = "Extract and decrypt cookies from macOS Chromium apps for VM cookie sync";
    license = lib.licenses.mit;
    mainProgram = "chromium-cookies";
    platforms = lib.platforms.darwin;
  };
}
