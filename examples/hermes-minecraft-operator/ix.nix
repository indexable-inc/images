{ index }:

# A Hermes agent operating a Paper Minecraft server. Two nodes:
#
#   minecraft (Paper + RCON)  <-east-west-  hermes (agent + RCON MCP server)
#
# The agent gets one typed `run_command` MCP tool that speaks RCON to
# the game server, so "whitelist my friend", "shrink the world border",
# or a daily player-count report are chat requests instead of console
# sessions. See README.md for the workflows.
let
  # RCON is only routable inside this group; the public internet sees
  # the game port (the minecraft node has ipv4 for player traffic), not
  # the console.
  eastWestGroup = "hermes-minecraft";
in
index.lib.mkFleet {

  nodes = {
    minecraft = {
      groups = [ eastWestGroup ];
      # Every declared ix.healthChecks entry (the minecraft module's unit
      # check included) is serialized into the plan and waited on by
      # `ix fleet up`; there is no per-node selector (ENG-2416).
      deployment.ipv4 = true;
      modules = [ ./minecraft.nix ];
    };

    hermes = {
      dependsOn = [ "minecraft" ];
      groups = [ eastWestGroup ];
      modules = [
        index.lib.hermesAgent.nixosModules.default
        (index.lib.paths.examples + "/hermes-agent/hermes.nix")
        ./operator.nix
      ];
    };
  };
}
