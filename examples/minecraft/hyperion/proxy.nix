# A VM players connect to. It terminates the Minecraft connection and forwards
# bytes to the game server without reading them, so this is the only part of
# the fleet with a public address.
{
  config,
  hyperionProxy,
  ix,
  lib,
  nodes,
  ...
}: let
  # Resolved from the game server's own `ix.networking.expose` declaration, so
  # a port change there reaches here without a second edit, and a typo in the
  # listener name is an evaluation error rather than a connection refused.
  game = ix.endpointOf nodes.hyperion-game "hyperion";

  # The full name, not the bare host and not an address. The proxy uses this
  # host as the TLS server name it expects on the game server's certificate,
  # so it has to be exactly the name that certificate was issued for.
  gameFqdn = "${game.host}.ix.internal";
in {
  hyperion.pki.serverName = "${config.ix.networking.eastWest.hostName}.ix.internal";

  ix.networking.expose.minecraft = {
    port = 25565;
    description = "public Minecraft entrypoint";
  };

  ix.healthChecks.hyperion-proxy.unit = "hyperion-proxy.service";

  # Pinned only when the group is unavailable. Group members are supposed to
  # resolve each other by name, and the name is load bearing: the proxy uses it
  # as the TLS server name, so substituting an address here would fail the
  # handshake. Mapping the name to an address in /etc/hosts keeps the name and
  # skips the resolver.
  networking.hosts = lib.mkIf (config.hyperion.gameAddress != null) {
    ${config.hyperion.gameAddress} = [gameFqdn];
  };

  services.hyperion-proxy = {
    enable = true;
    package = hyperionProxy;
    listen = "[::]:25565";
    gameServer = "${gameFqdn}:${toString game.port}";
    pki = {
      rootCaCert = "/var/lib/hyperion-pki/root_ca.crt";
      cert = "/var/lib/hyperion-pki/node.crt";
      privateKey = "/var/lib/hyperion-pki/node_private_key.pem";
    };
  };
}
