{
  id = "minecraft-sound";
  packageSet = true;
  flake = true;
  inRustWorkspace = true;
  passthruTests = true;
  # The overlay eval context has no `ix`, so build via `buildIxRustTool` (which
  # injects ix.rustWorkspace/cargoUnit) like oci-image-builder. default.nix then
  # produces the sound-pack-wrapped binary. Surfaces as pkgs.minecraft-sound.
  overlay = {
    attrName = "minecraft-sound";
    build = {
      buildIxRustTool,
      final,
      path,
      ...
    }:
      buildIxRustTool final path;
  };
}
