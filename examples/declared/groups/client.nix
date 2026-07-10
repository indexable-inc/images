{
  ix,
  lib,
  nodes,
  pkgs,
  ...
}: let
  # Resolve the api node's listener by the name it exposes it under.
  api = ix.endpointOf nodes.api "http";
in {
  environment.systemPackages = [pkgs.curl];

  ix.healthChecks.private-api = {
    description = "image-declared group gives the client a private path to the api";
    command = [
      (lib.getExe pkgs.curl)
      "--fail"
      "--silent"
      "--show-error"
      "http://${api}/"
    ];
  };
}
