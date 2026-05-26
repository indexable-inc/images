# SpongeVanilla server jar. https://spongepowered.org
# Standalone Sponge implementation (no Forge needed).
{
  ix,
  config,
  lib,
  ...
}:
ix.mkMinecraftLoader {
  inherit ix config lib;
  name = "sponge";
  dropinDir = "mods";
}
