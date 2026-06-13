{
  lib,
  pkgs,
  ...
}:
let
  httpPort = 8080;
in
{
  # The image carries its own east-west membership: every fleet that
  # deploys this image lands in the deployer's `declared-groups` network
  # without a fleet-level `groups` entry.
  ix.networking.groups = [ "declared-groups" ];

  services.nginx = {
    enable = true;
    virtualHosts."declared-groups" = {
      default = true;
      listen = [
        {
          addr = "0.0.0.0";
          port = httpPort;
        }
      ];
      locations."/".return = "200 'declared-groups private api\n'";
    };
  };

  environment.systemPackages = [ pkgs.curl ];

  ix.networking.expose.http = {
    port = httpPort;
    description = "private HTTP API for east-west group members";
  };

  ix.healthChecks = {
    nginx.unit = "nginx";

    http-loopback = {
      description = "private HTTP API answers locally";
      command = [
        (lib.getExe pkgs.curl)
        "--fail"
        "--silent"
        "--show-error"
        "http://127.0.0.1:${toString httpPort}/"
      ];
    };
  };
}
