{nodes, ...}: let
  ports = import ./ports.nix;
  # Join by the server's east-west hostname, so the reference stays correct
  # regardless of which IP the server lands on.
  serverHost = nodes.k3s-server.config.ix.networking.eastWest.hostName;
in {
  imports = [./node.nix];

  services.k3s = {
    role = "agent";
    serverAddr = "https://${serverHost}:${toString ports.api}";
  };
}
