# Fabric server jar. https://fabricmc.net
# Server jar comes from `ix.artifacts.minecraft.servers."${version}-fabric"`;
# the Fabric loader and installer versions are baked into the upstream URL
# pinned in lib, not surfaced to consumers.
{
  ix,
  config,
  lib,
  pkgs,
  ...
}:
let
  defaultJvmVersion = import ../../../../lib/languages/jvm-defaults.nix;
in
ix.mkMinecraftLoader {
  inherit ix config lib;
  name = "fabric";
  dropinDir = "mods";
  # Fabric uses the shared Temurin JVM default. Hot reload can redefine ordinary
  # classes through the Java agent, but it does not dynamically load new mods or
  # mutate frozen registries.
  configFragment = _: {
    services.minecraft.javaPackage = lib.mkDefault pkgs."temurin-jre-bin-${defaultJvmVersion}";
  };
}
