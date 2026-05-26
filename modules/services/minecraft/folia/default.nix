# Folia server jar. https://papermc.io/software/folia
# PaperMC fork for regionized multithreading.
{
  ix,
  config,
  lib,
  ...
}:
ix.mkMinecraftLoader {
  inherit ix config lib;
  name = "folia";
  dropinDir = "plugins";
}
