# Distant Horizons: server-side LOD generation for clients running DH.
#
# Activated when `services.minecraft.mods.distanthorizons` is set.
# Generates DistantHorizons.toml from the user's attrset (with defaults).
{ config, lib, ... }:
let
  modCfg = config.services.minecraft.mods.distanthorizons or null;
  modEnabled = modCfg != null && modCfg.enable;
  defaults = {
    serverSideLodGeneration = true;
    maxRenderDistance = 256;
  };
  modSettings = if modCfg == null then { } else builtins.removeAttrs modCfg [ "enable" ];
  merged = defaults // modSettings;
in
{
  config = lib.mkIf modEnabled {
    services.minecraft.configFiles."DistantHorizons.toml" = {
      server = {
        inherit (merged) serverSideLodGeneration maxRenderDistance;
      };
    };
  };
}
