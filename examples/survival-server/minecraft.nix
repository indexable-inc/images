_: {
  services = {
    velocity = {
      enable = true;
      motd = "<green>ix Survival</green> <gray>| Java and Bedrock</gray>";
      # Paper picks this up automatically: services.minecraft.paper defaults
      # paper-global.yml's proxies.velocity block from the same string.
      forwarding.secret = "ix-survival-example-forwarding-secret-change-me";
      servers.survival = "127.0.0.1:25566";
      try = [ "survival" ];
    };

    geyser = {
      enable = true;
      bedrock = {
        motd1 = "ix Survival";
        motd2 = "Java and Bedrock";
        serverName = "ix Survival";
      };
    };

    floodgate.enable = true;

    minecraft = {
      enable = true;
      version = "26.1.2";
      paper.enable = true;
      port = 25566;
      openFirewall = false;

      properties = {
        motd = "ix Survival";
        difficulty = "hard";
        gamemode = "survival";
        level-name = "survival";
        max-players = 120;
        online-mode = false;
        spawn-protection = 0;
        view-distance = 16;
        simulation-distance = 10;
        pvp = true;
      };

      serverFiles."spigot.yml".settings = {
        bungeecord = false;
        restart-on-crash = false;
      };
    };
  };
}
