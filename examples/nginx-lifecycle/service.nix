{
  config,
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

  networking.firewall.allowedTCPPorts = [ nginxPort ];
  environment.systemPackages = [ pkgs.curl ];

  ix.networking.portClaims.nginx = {
    protocol = "tcp";
    port = nginxPort;
    description = "nginx lifecycle HTTP";
  };

  ix.healthChecks = {
    nginx = {
      command = [
        (lib.getExe' config.systemd.package "systemctl")
        "is-active"
        "--quiet"
        "nginx.service"
      ];
    };

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
