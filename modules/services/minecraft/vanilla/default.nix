# Vanilla server jar. https://www.minecraft.net
# Mojang's manifest is dynamic; callers pass the locked server jar artifact
# as `services.minecraft.vanilla.src`.
{
  ix,
  config,
  lib,
  ...
}:
ix.mkMinecraftLoader {
  inherit ix config lib;
  name = "vanilla";
}
