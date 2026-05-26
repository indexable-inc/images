# LuckPerms: permissions management.
#
# Activated when `services.minecraft.mods.luckperms` is set.
# Optionally provisions MariaDB for permission storage when `mysql = true`.
{
  config,
  lib,
  pkgs,
  ...
}:
let
  modCfg = config.services.minecraft.mods.luckperms or null;
  modEnabled = modCfg != null && modCfg.enable;
  defaults = {
    mysql = false;
  };
  modSettings = if modCfg == null then { } else builtins.removeAttrs modCfg [ "enable" ];
  merged = defaults // modSettings;
in
{
  config = lib.mkIf modEnabled {
    services.mysql = lib.mkIf merged.mysql {
      enable = true;
      package = lib.mkDefault pkgs.mariadb;
      ensureDatabases = [ "luckperms" ];
      ensureUsers = [
        {
          name = "minecraft";
          ensurePermissions = {
            "luckperms.*" = "ALL PRIVILEGES";
          };
        }
      ];
    };

    services.minecraft.configFiles."LuckPerms/config.yml" = lib.mkIf merged.mysql {
      storage-method = "mysql";
      data = {
        address = "localhost:3306";
        database = "luckperms";
        username = "minecraft";
        password = "";
      };
    };
  };
}
