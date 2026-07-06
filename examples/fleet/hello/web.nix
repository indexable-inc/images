{
  lib,
  pkgs,
  ...
}: let
  httpPort = 8080;
in {
  services.nginx = {
    enable = true;
    virtualHosts."fleet-hello" = {
      default = true;
      listen = [
        {
          addr = "0.0.0.0";
          port = httpPort;
        }
      ];
      locations."/".return = "200 'hello from the fleet\n'";
    };
  };

  # One declaration opens the firewall, registers the port claim, and lets
  # workers resolve this listener with `ix.endpointOf nodes.web "http"`.
  ix.networking.expose.http = {
    port = httpPort;
    description = "hello HTTP service for the fleet's workers";
  };

  ix.healthChecks = {
    nginx.unit = "nginx";

    http-loopback = {
      description = "hello HTTP service answers locally";
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
