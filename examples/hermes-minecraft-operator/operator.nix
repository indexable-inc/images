# Operator deltas on top of the shared hermes-agent composition: the
# RCON MCP server pointed at the minecraft node, and a persona that
# knows it is running a game server.
{
  ix,
  lib,
  nodes,
  pkgs,
  ...
}:
let
  rcon = import ./rcon.nix;

  # The minecraft node's east-west name; resolvable because both nodes
  # share the fleet's group. The RCON port is read off the sibling's
  # evaluated config so a port change there cannot strand this side.
  rconHost = nodes.minecraft.config.ix.networking.eastWest.hostName;
  rconPort = nodes.minecraft.config.services.minecraft.rcon.port;

  # The MCP stdio server: one typed `run_command(command) -> response`
  # tool speaking Source RCON. Typed is the point: the agent hands the
  # game server a console-command string and nothing else; no argv, no
  # shell, no file paths. The Minecraft server's own command grammar
  # and permission model do the parsing.
  rconMcp = ix.writePythonApplication pkgs {
    name = "minecraft-rcon-mcp";
    src = ./mcp/rcon_mcp.py;
    meta.description = "MCP stdio server exposing Minecraft RCON as a typed run_command tool";
  };
in
{
  services.hermes-agent = {
    mcpServers.minecraft = {
      command = lib.getExe rconMcp;
      env = {
        RCON_HOST = rconHost;
        RCON_PORT = toString rconPort;
        # The shared example credential (see rcon.nix). Scoped by the
        # east-west group, named change-me on purpose.
        RCON_PASSWORD = rcon.password;
      };
    };

    # The shared composition binds its operator persona with mkDefault,
    # so a plain assignment swaps in the game-server persona.
    documents."SOUL.md" = ./documents/SOUL.md;
  };
}
