# The game server node: Paper with RCON enabled for the agent. Shaped
# like examples/minecraft/survival minus the proxy stack; the one
# operator-specific piece is the seeded RCON password shared with the
# hermes node (see rcon.nix).
{lib, ...}: let
  rcon = import ./rcon.nix;
in {
  services.minecraft = {
    enable = true;
    version = "26.1.2";
    paper.enable = true;
    openFirewall = true;

    properties = {
      motd = "ix Hermes-operated server";
      difficulty = "normal";
      gamemode = "survival";
      level-name = "operator";
      max-players = 40;
      online-mode = false;
      spawn-protection = 0;
    };

    # The agent manages the whitelist over RCON (`whitelist add <name>`),
    # so the server enforces it from first boot.
    whitelist.enable = true;

    rcon = {
      enable = true;
      inherit (rcon) port;
      # Reachable from the hermes node only through the shared east-west
      # group; the group, not this flag, is the security boundary.
      openFirewall = true;
    };
  };

  # Seed the shared RCON password before the module's own first-start
  # generation runs ("generated when absent"), so both nodes agree on
  # the credential without any runtime exchange.
  systemd.services.minecraft.preStart = lib.mkBefore ''
    if [ ! -s /var/lib/minecraft/.ix-rcon-password ]; then
      umask 077
      mkdir -p /var/lib/minecraft
      printf '%s' ${lib.escapeShellArg rcon.password} > /var/lib/minecraft/.ix-rcon-password
    fi
  '';
}
