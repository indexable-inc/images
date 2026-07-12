{
  ix,
  lib,
  nodes,
  pkgs,
  ...
}: let
  # Resolve the service node's listener by the name it exposes it under.
  service = ix.endpointOf nodes.service "http";
in {
  environment.systemPackages = [pkgs.curl];

  ix.healthChecks.private-service = {
    description = "private service is reachable from an east-west group member";
    command = [
      (lib.getExe pkgs.curl)
      "--fail"
      "--silent"
      "--show-error"
      "http://${service}/"
    ];
  };
}
