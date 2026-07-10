/**
Declare a Minecraft loader module (Fabric, Paper, Vanilla, ...).

Each loader is structurally identical: it owns
`services.minecraft.<name>` with an enable flag and a server-jar slot,
and assigns that jar to `services.minecraft.serverJar`. The `src` slot
defaults to `ix.artifacts.minecraft.servers."${cfg.version}-${name}"`,
so callers that set `services.minecraft.version` rarely override
anything per loader.

Reached from modules via `specialArgs.ix.mkMinecraftLoader`. Loader
files call it and return the resulting module attrset directly.
Loaders that need to contribute more to `config` pass a
`configFragment cfg` hook; it merges into the gated config so the loader
file stays a single expression.
*/
{
  ix,
  config,
  lib,
  name,
  dropinDir ? "mods",
  extraOptions ? {},
  configFragment ? _: {},
}: let
  cfg = config.services.minecraft.${name};
  minecraftCfg = config.services.minecraft;
  inherit (ix.artifacts.minecraft) servers;
  versionKey =
    if minecraftCfg.version == null
    then null
    else "${minecraftCfg.version}-${name}";
  defaultSrc =
    if versionKey != null && servers ? ${versionKey}
    then servers.${versionKey}
    else throw "services.minecraft.${name}.src has no default: set `services.minecraft.version` to a value with a pinned `${name}` artifact in `ix.artifacts.minecraft.servers`, or set `services.minecraft.${name}.src` explicitly.";
in {
  options.services.minecraft.${name} =
    {
      enable = lib.mkEnableOption "${name} server jar";
      src = lib.mkOption {
        type = lib.types.path;
        default = defaultSrc;
        defaultText = lib.literalExpression ''ix.artifacts.minecraft.servers."''${services.minecraft.version}-${name}"'';
        description = "Locked server jar artifact. Defaults to the pinned artifact for `services.minecraft.version`.";
      };
    }
    // extraOptions;

  config = lib.mkIf cfg.enable (
    lib.mkMerge [
      {
        services.minecraft = {
          enable = lib.mkDefault true;
          dropinDir = lib.mkDefault dropinDir;
          serverJar = cfg.src;
        };
      }
      (configFragment cfg)
    ]
  );
}
