{
  lib,
  pkgs,
  ...
}: let
  httpPort = 8080;
in {
  services.nginx = {
    enable = true;
    virtualHosts."east-west-firewall" = {
      default = true;
      listen = [
        {
          addr = "0.0.0.0";
          port = httpPort;
        }
      ];
      locations."/".return = "200 'east-west private service\n'";
    };
  };

  environment.systemPackages = [pkgs.curl];

  # One declaration opens the firewall, registers the claim, and lets
  # east-west peers resolve this listener with `ix.endpointOf nodes.service "http"`.
  ix.networking.expose.http = {
    port = httpPort;
    description = "private HTTP service for east-west group members";
  };

  ix.healthChecks = {
    nginx.unit = "nginx";

    http-loopback = {
      description = "private HTTP service answers locally";
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
