_:
let
  forwardingSecret = "ix-survival-example-forwarding-secret-change-me";
in
{
  services = {
    velocity = {
      enable = true;
      motd = "<green>ix Survival</green> <gray>| Java and Bedrock</gray>";
      # Velocity renders MiniMessage tags into plain text before answering
      # SLP. Substring matching here proves the proxy is actually the one
      # responding on 25565, not a stray backend or stale image.
      health.motdContains = [ "ix Survival" ];
      forwarding.secret = forwardingSecret;
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

      configFiles."paper-global.yml".proxies.velocity = {
        enabled = true;
        secret = forwardingSecret;
        online-mode = true;
      };

      serverFiles."spigot.yml".settings = {
        bungeecord = false;
        restart-on-crash = false;
      };
    };
  };
}
