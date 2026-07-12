{
  ix,
  lib,
  nodes,
  pkgs,
  ...
}: let
  service = ix.endpointOf nodes.service "http";
in {
  environment.systemPackages = [pkgs.curl];

  ix.healthChecks.private-service-denied = {
    description = "private service is unreachable outside the east-west group";
    attempts = 3;
    command = [
      (lib.getExe pkgs.bash)
      "-e"
      "-u"
      "-o"
      "pipefail"
      "-c"
      ''
        if ${lib.getExe pkgs.curl} --fail --silent --show-error --connect-timeout 2 http://${service}/; then
          echo "unexpectedly reached http://${service}/" >&2
          exit 1
        fi
      ''
    ];
  };
}
