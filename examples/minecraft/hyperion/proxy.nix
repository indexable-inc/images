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

  ix = {
    networking = {
      # `ipv4 = true` belongs here and is deliberately off, because in
      # `us-west-1` today it does not add an address, it removes the VM from
      # the network.
      #
      # The region's ingress block `15.204.22.192/26` is not delivered to the
      # OVH vRack: its gateway `15.204.22.254` is `FAILED` in the host's
      # neighbour table. A VM with no public address sources from `10.0.0.x`,
      # is masqueraded, and leaves over the host's own uplink. A VM that takes
      # one sources from the public block, matches the host's
      # `from 15.204.22.192/26 iif br-north lookup 200` rule, and is handed to
      # that dead gateway, so everything it sends is blackholed. Both proxies
      # came up unable to resolve, unable to substitute, and unable to complete
      # their switch at all. ENG-10881.
      #
      # Turn it back on, and recreate the proxies, once OVH delivers the block.
      # A public IPv4 is what this example wants: the Java client resolves
      # through the JDK, which prefers IPv4, and a large share of players have
      # no IPv6 at all.

      expose.minecraft = {
        port = 25565;
        description = "public Minecraft entrypoint";
      };
    };

    healthChecks.hyperion-proxy.unit = "hyperion-proxy.service";
  };

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
