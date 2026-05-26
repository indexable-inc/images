# NeoForge server jar. https://neoforged.net
# NeoForge uses an installer that generates server files, so this takes a
# locked artifact for the final server jar.
{
  ix,
  config,
  lib,
  ...
}:
ix.mkMinecraftLoader {
  inherit ix config lib;
  name = "neoforge";
  dropinDir = "mods";
}
