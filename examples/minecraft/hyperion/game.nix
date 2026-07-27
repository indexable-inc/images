# The VM holding the world. No public address: the proxies dial in over the
# group, and a VM outside the group has no route here at all.
{
  config,
  hyperionGameServer,
  ...
}: let
  port = 35565;
  # The name proxies reach this VM by, and therefore the name its certificate
  # has to be issued for. Group members resolve each other as
  # `<eastWest.hostName>.ix.internal`.
  fqdn = "${config.ix.networking.eastWest.hostName}.ix.internal";
in {
  hyperion.pki.serverName = fqdn;

  # One declaration of "this VM listens here", which is also what
  # `ix.endpointOf` reads on the proxy side.
  ix.networking.expose.hyperion = {
    inherit port;
    description = "hyperion game server, dialled by the proxies over the group";
  };

  ix.healthChecks.hyperion-game-server.unit = "hyperion-game-server.service";

  services.hyperion-game-server = {
    enable = true;
    package = hyperionGameServer;
    inherit port;
    pki = {
      rootCaCert = "/var/lib/hyperion-pki/root_ca.crt";
      cert = "/var/lib/hyperion-pki/node.crt";
      privateKey = "/var/lib/hyperion-pki/node_private_key.pem";
    };
  };
}
