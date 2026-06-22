# Versioned overlays for the minecraft image.
#
# Discovery exposes `minecraft_<key>` for every version key and `minecraft` as
# an alias for the version named in `default`.
#
# Each variant just declares its loader, its Minecraft version, and the mod
# slugs it wants enabled. The loader module derives the server jar from
# `ix.artifacts.minecraft.servers."${version}-${loader}"`, and the runtime
# module defaults `modCatalog` to the matching version catalog plus the
# `common` cross-version mods.
{ lib, ... }:
let
  default = "26.1.2-fabric";

  variants = {
    "26w17a-fabric" = {
      loader = "fabric";
      version = "26.2-snapshot-5";
      mods = [
        "fabric-api"
        "c2me-fabric"
      ];
    };

    "26.1.2-fabric" = {
      loader = "fabric";
      version = "26.1.2";
      mods = [
        "fabric-api"
        "lithium"
        "c2me-fabric"
        "krypton"
        "ferrite-core"
        "servercore"
        "vmp-fabric"
        "clumps"
        "spark"
        "grimac"
      ];
    };

    "26.1.2-paper" = {
      loader = "paper";
      version = "26.1.2";
      mods = [ ];
    };

    "1.21.11-fabric" = {
      loader = "fabric";
      version = "1.21.11";
      mods = [
        "fabric-api"
        "spark"
        "terrain-diffusion"
      ];
    };

    "1.21.11-paper" = {
      loader = "paper";
      version = "1.21.11";
      mods = [ ];
    };
  };

  mkVariant =
    tag:
    {
      loader,
      version,
      mods,
    }:
    {
      ix.image.tag = tag;
      services.minecraft = {
        inherit version;
        mods = lib.genAttrs mods (_: { });
        ${loader}.enable = true;
      };
    };
in
{
  inherit default;
}
// lib.mapAttrs mkVariant variants
