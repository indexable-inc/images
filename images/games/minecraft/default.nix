# Minecraft server image.
#
# This file is the version-agnostic base. Per-version data lives in
# `./versions.nix` as overlay modules layered on top of this one by
# `lib.discoverImages`.
#
# Every variant auto-enables the cross-version `common` mods so an image
# always ships the baseline performance/QoL set. The version overlay sets
# `services.minecraft.version`, which drives the server jar (via the
# loader's `src` default) and the per-version slice of `modCatalog`.
{ ix, lib, ... }:
let
  commonCatalog = ix.artifacts.minecraft.modCatalogs.common;
in
{
  ix.image.name = "minecraft";

  services.minecraft = {
    enable = true;
    properties.motd = "ix-powered Minecraft";
    mods = lib.genAttrs (lib.attrNames commonCatalog) (_: { });
  };
}
