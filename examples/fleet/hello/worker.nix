{
  ix,
  lib,
  nodes,
  pkgs,
  ...
}: let
  # Resolve the web node's listener by the name it exposes it under.
  web = ix.endpointOf nodes.web "http";
in {
  environment.systemPackages = [pkgs.curl];

  ix.healthChecks.web-reachable = {
    description = "web service is reachable from this worker";
    command = [
      (lib.getExe pkgs.curl)
      "--fail"
      "--silent"
      "--show-error"
      "http://${web}/"
    ];
  };
}
