{lib, ...}: let
  world = import ./world.nix;
  plugins = import ./plugins.nix {inherit world;};
in {
  services.minecraft = {
    enable = true;
    version = "26.1.2";
    paper.enable = true;

    # Local RCON is required for the managed world border and PlugManX reloads.
    # It stays off the firewall unless rcon.openFirewall is set.
    rcon = {
      enable = true;
      broadcastToOps = false;
    };

    autoReload.plugman.ignoredPlugins = lib.mkAfter plugins.autoReloadIgnored;

    # Uncomment and add real Minecraft UUIDs to derive whitelist.json and
    # ops.json from one player record.
    # whitelist.enable = true;
    # players = {
    #   Alice = {
    #     uuid = "00000000-0000-0000-0000-000000000000";
    #     whitelist = true;
    #     operator.enable = true;
    #   };
    # };

    plugins = plugins.enabled;

    datapacks."max-height" = {
      inherit (world.height) dimensionTypes pack;
      worlds = [world.name];
    };

    properties = {
      motd = "ix Factions | territory, raids, shops";
      difficulty = "hard";
      gamemode = "survival";
      level-name = world.name;
      level-seed = world.seed;
      max-players = 250;
      spawn-protection = 0;
      view-distance = 16;
      simulation-distance = 10;
      max-world-size = world.border.radius;
      pvp = true;
    };

    worldBorder = {
      enable = true;
      inherit (world.border) diameter;
      center = {
        x = 0;
        z = 0;
      };
      warning = {
        distance = 64;
        time = 15;
      };
      damage = {
        buffer = 16;
        amount = 0.2;
      };
    };

    bukkit = import ./bukkit.nix;
    configFiles = import ./paper.nix;
    serverFiles."spigot.yml" = import ./spigot.nix;
  };
}
