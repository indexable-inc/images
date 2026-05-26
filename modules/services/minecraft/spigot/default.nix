# Spigot server jar. https://www.spigotmc.org
# CraftBukkit fork. No direct download API (official method is BuildTools),
# so callers pass the locked server jar artifact as `src`.
{
  ix,
  config,
  lib,
  ...
}:
ix.mkMinecraftLoader {
  inherit ix config lib;
  name = "spigot";
  dropinDir = "plugins";
}
