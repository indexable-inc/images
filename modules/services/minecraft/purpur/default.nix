# Purpur server jar. https://purpurmc.org
# Paper fork with extra gameplay and performance patches.
{
  ix,
  config,
  lib,
  ...
}:
ix.mkMinecraftLoader {
  inherit ix config lib;
  name = "purpur";
  dropinDir = "plugins";
}
