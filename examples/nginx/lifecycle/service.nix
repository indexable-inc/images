{
  lib,
  pkgs,
  ...
}:
let
  nginxPort = 80;
in
{
  services.nginx = {
    enable = true;
    virtualHosts.localhost.locations."/".return = "200 'ix nginx lifecycle ok\n'";
  };

  environment.systemPackages = [ pkgs.curl ];

  ix.networking.expose.nginx = {
    port = nginxPort;
    description = "nginx lifecycle HTTP";
  };

  ix.healthChecks = {
    nginx.unit = "nginx";

    nginx-http = {
      description = "nginx HTTP loopback";
      command = [
        (lib.getExe pkgs.curl)
        "--fail"
        "--silent"
        "--show-error"
        "http://127.0.0.1/"
      ];
    };
  };
}
